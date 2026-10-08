//! `obc data runs`: the runs in the store, the steps of one run, and its events as they come.

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::BTreeMap;

use crate::engine::runs::{self, Details, Event, Outcome, RunStep, Summary};
use crate::store::Store;

use super::{cells, print_json, print_table, Code, Error};

#[derive(Args)]
pub struct Runs {
    /// A run id: show its steps with their inputs, code and users.
    run: Option<String>,
    /// Write the events of RUN as they come, until it ends. The exit status is 1 when it failed.
    #[arg(long, requires = "run")]
    follow: bool,
    /// Stop admitting work and drain current work. An apply stops before its next phase.
    #[arg(long, requires = "run", conflicts_with_all = ["follow", "result"])]
    stop: bool,
    /// Show the completed operation output. An unresolved run has no result.
    #[arg(long, requires = "run", conflicts_with = "follow")]
    result: bool,
}

pub fn run(args: Runs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let Some(id) = args.run else {
        return list(&store, json);
    };
    runs::check_id(&id).map_err(|e| Code::Usage.error(e))?;
    if !store.run(&id).is_file() {
        return Err(Code::Usage.error(format!("no run `{id}`")));
    }
    if crate::operation::read(&store, &id)?.is_some() {
        if args.stop {
            crate::operation::stop(&store, &id).map_err(|message| Code::Blocked.error(message).with_run(&id))?;
        }
        if args.follow {
            return watch(&store, &id, json);
        }
        let view = super::operation_cli::view(&store, &id)?;
        if args.result {
            if !matches!(view.operation, Some(crate::operation::Status::Finished { .. })) {
                return Err(Code::Blocked.error("the operation has no final result yet").with_run(&id));
            }
            return print_json(
                &view.result.ok_or_else(|| {
                    Code::Blocked.error("the final worker has not sealed its output yet").with_run(&id)
                })?,
            );
        }
        return show_view(&view, json);
    }
    if args.stop || args.result {
        return Err(Code::Usage.error("this run has no detached operation"));
    }
    if args.follow {
        return follow(&store, &id, json);
    }
    show(&runs::details(&store, &id)?, json)
}

/// Every run in the store, newest first.
#[derive(Serialize, JsonSchema)]
pub struct RunList<'a> {
    runs: &'a [Summary],
    operations: BTreeMap<String, crate::operation::Status>,
    observation_errors: BTreeMap<String, String>,
}

fn list(store: &Store, json: bool) -> Result<(), Error> {
    let mut summaries = runs::list(store)?;
    let mut operations = BTreeMap::new();
    let mut observation_errors = BTreeMap::new();
    for summary in &mut summaries {
        if crate::operation::read(store, &summary.id)?.is_some() {
            let view = super::operation_cli::view(store, &summary.id)?;
            *summary = view.run.summary;
            if let Some(status) = view.operation {
                operations.insert(summary.id.clone(), status);
            }
            if let Some(error) = view.observation_error {
                observation_errors.insert(summary.id.clone(), error);
            }
        }
    }
    let runs = summaries.as_slice();
    if json {
        return print_json(&RunList { runs, operations, observation_errors });
    }
    let mut table = vec![cells(["RUN", "COMMAND", "", "TOOK", "FETCHED", "BUILT"])];
    for run in runs {
        let took = run.wall_ms.map_or("—".into(), duration);
        let (command, mark) =
            (run.command.clone(), operations.get(&run.id).map_or_else(|| mark(run.outcome).into(), state));
        table.push(vec![run.id.clone(), command, mark, took, bytes(run.bytes_fetched), bytes(run.bytes_built)]);
    }
    print_table(&table);
    Ok(())
}

fn show(run: &Details, json: bool) -> Result<(), Error> {
    if json {
        return print_json(run);
    }
    let summary = &run.summary;
    let took = summary.wall_ms.map_or("—".into(), duration);
    println!("{}  {}  {}  {took}", summary.id, summary.command, mark(summary.outcome));
    if let Some(phase) = run.phase {
        println!("phase {phase:?}; {} remote writes acknowledged", run.published.len());
    }
    if let Some(error) = &run.error {
        println!("{error}");
    }
    if !run.fetches.is_empty() {
        let mut table = vec![cells(["FETCH", "TOOK", "SIZE"])];
        for fetch in &run.fetches {
            let took = match (&fetch.error, fetch.wall_ms) {
                (Some(error), _) => format!("✗ {error}"),
                (None, Some(wall_ms)) => duration(wall_ms),
                (None, None) => "running".into(),
            };
            table.push(vec![
                fetched(&fetch.source, &fetch.version, &fetch.params),
                took,
                fetch.bytes.map_or("—".into(), bytes),
            ]);
        }
        print_table(&table);
    }
    let mut table = vec![cells(["STEP", "TOOK", "Δ LAST RUN", "PEAK RAM", "OUTPUT", "READS", "CODE", "USED BY"])];
    for step in &run.steps {
        let receipt = step.receipt.as_ref();
        let [took, change, ram, output] = step_cells(step);
        let reads = receipt.map(|r| r.inputs.iter().map(|input| input.name.as_str()).collect::<Vec<_>>().join(", "));
        table.push(vec![
            step.step.clone(),
            took,
            change,
            ram,
            output,
            reads.unwrap_or_default(),
            receipt.map(|receipt| receipt.code[..12].to_string()).unwrap_or_default(),
            step.users.join(", "),
        ]);
    }
    if !run.steps.is_empty() {
        print_table(&table);
    }
    Ok(())
}

fn follow(store: &Store, id: &str, json: bool) -> Result<(), Error> {
    let mut ok = false;
    runs::follow(store, id, |event| {
        if let Event::Finished { ok: finished, .. } = event {
            ok = *finished;
        }
        if json {
            println!("{}", serde_json::to_string(event).map_err(|e| e.to_string())?);
            return Ok(());
        }
        println!(
            "{}",
            match event {
                Event::Started { command, at } => format!("started {command} at {at}"),
                Event::Phase { phase } => format!("phase {phase:?}"),
                Event::Published { mutation } => serde_json::to_string(mutation).unwrap(),
                Event::StepStarted { step } => format!("{step} started"),
                Event::StepFinished { step, reused: true, .. } => format!("{step} reused"),
                Event::StepFinished { step, receipt, .. } => {
                    format!("{step} built in {}, {}", duration(receipt.wall_ms), bytes(receipt.bytes_out))
                }
                Event::StepFailed { step, error } => format!("{step} failed: {error}"),
                Event::FetchStarted { source, version, params } => {
                    format!("{} fetch started", fetched(source, version, params))
                }
                Event::FetchFinished { source, version, params, resolved, bytes: size, wall_ms, .. } => {
                    format!(
                        "{} fetched {resolved} in {}, {}",
                        fetched(source, version, params),
                        duration(*wall_ms),
                        bytes(*size)
                    )
                }
                Event::FetchFailed { source, version, params, error, .. } => {
                    format!("{} fetch failed: {error}", fetched(source, version, params))
                }
                Event::Finished { ok: true, wall_ms, .. } => format!("finished in {}", duration(*wall_ms)),
                Event::Finished { error, .. } =>
                    format!("failed: {}", error.as_deref().unwrap_or("no reason recorded")),
            }
        );
        Ok(())
    })?;
    if !ok {
        return Err(Code::RunFailed.error(format!("run {id} failed")));
    }
    Ok(())
}

fn state(status: &crate::operation::Status) -> String {
    use crate::operation::Status;
    match status {
        Status::Starting => "starting",
        Status::Running => "running",
        Status::Stopping => "stopping after current work",
        Status::Stopped => "stopped",
        Status::Interrupted => "interrupted",
        Status::Finished { ok: true } => "complete",
        Status::Finished { ok: false } => "failed",
    }
    .into()
}

fn show_view(view: &super::operation_cli::View, json: bool) -> Result<(), Error> {
    if json {
        return print_json(view);
    }
    if let Some(status) = &view.operation {
        println!("{}: {}", view.run.summary.id, state(status));
    }
    if let Some(error) = &view.observation_error {
        println!("{error}");
    }
    show(&view.run, false)
}

fn watch(store: &Store, run: &str, json: bool) -> Result<(), Error> {
    let mut previous = String::new();
    loop {
        let view = super::operation_cli::view(store, run)?;
        let current = serde_json::to_string(&view).map_err(|e| e.to_string())?;
        if current != previous {
            show_view(&view, json)?;
            previous = current;
        }
        match view.operation {
            Some(crate::operation::Status::Finished { ok: true }) => return Ok(()),
            Some(
                crate::operation::Status::Finished { ok: false }
                | crate::operation::Status::Stopped
                | crate::operation::Status::Interrupted,
            ) => return Err(Code::RunFailed.error("operation did not complete").with_run(run)),
            _ => {}
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

/// The time of a step, its change since the last run that built it, its peak RAM and its output.
pub(super) fn step_cells(step: &RunStep) -> [String; 4] {
    let receipt = step.receipt.as_ref();
    let took = match receipt {
        Some(_) if step.reused => "reused".into(),
        Some(receipt) => duration(receipt.wall_ms),
        None if step.error.is_some() => "—".into(),
        None => "running".into(),
    };
    let change = match (receipt.filter(|_| !step.reused), step.last_wall_ms) {
        (Some(receipt), Some(last)) if receipt.wall_ms == last => "=".into(),
        (Some(receipt), Some(last)) if receipt.wall_ms > last => format!("+{}", duration(receipt.wall_ms - last)),
        (Some(receipt), Some(last)) => format!("−{}", duration(last - receipt.wall_ms)),
        _ => "—".into(),
    };
    let ram = receipt.and_then(|receipt| receipt.peak_rss_bytes).map_or("—".into(), bytes);
    let output = match (&step.error, receipt) {
        (Some(error), _) => format!("✗ {error}"),
        (None, Some(receipt)) => bytes(receipt.bytes_out),
        (None, None) => "—".into(),
    };
    [took, change, ram, output]
}

/// `SOURCE@VERSION NAME=VALUE…`
fn fetched(source: &str, version: &str, params: &[(String, String)]) -> String {
    let params = params.iter().map(|(name, value)| format!(" {name}={value}"));
    format!("{source}@{version}{}", params.collect::<String>())
}

pub(super) fn mark(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Running => "◐",
        Outcome::Ok => "✓",
        Outcome::Failed => "✗",
    }
}

pub(super) fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0 => format!("{ms} ms"),
        1..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

pub(super) fn bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.1} kB", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        _ => format!("{:.2} GB", n as f64 / 1e9),
    }
}
