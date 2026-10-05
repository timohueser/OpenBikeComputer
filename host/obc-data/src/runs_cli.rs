//! `obc data runs`: the runs in the store, the steps of one run, and its events as they come.

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use obc_data::engine::runs::{self, Details, Event, Outcome, Summary};
use obc_data::store::Store;

use crate::{cells, print_json, print_table, Code, Error};

#[derive(Args)]
pub struct Runs {
    /// A run id: show its steps with their inputs, code and users.
    run: Option<String>,
    /// Write the events of RUN as they come, until it ends. The exit status is 1 when it failed.
    #[arg(long, requires = "run")]
    follow: bool,
}

pub fn run(args: Runs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let Some(id) = args.run else {
        return list(&runs::list(&store)?, json);
    };
    runs::check_id(&id).map_err(|e| Code::Usage.error(e))?;
    if !store.run(&id).is_file() {
        return Err(Code::Usage.error(format!("no run `{id}`")));
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
}

fn list(runs: &[Summary], json: bool) -> Result<(), Error> {
    if json {
        return print_json(&RunList { runs });
    }
    let mut table = vec![cells(["RUN", "COMMAND", "", "TOOK", "FETCHED", "BUILT"])];
    for run in runs {
        let took = run.wall_ms.map_or("—".into(), duration);
        let (command, mark) = (run.command.clone(), mark(run.outcome).into());
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
        let output = match (&step.error, receipt) {
            (Some(error), _) => format!("✗ {error}"),
            (None, Some(receipt)) => bytes(receipt.bytes_out),
            (None, None) => "—".into(),
        };
        let reads = receipt.map(|r| r.inputs.iter().map(|input| input.name.as_str()).collect::<Vec<_>>().join(", "));
        table.push(vec![
            step.step.clone(),
            took,
            change,
            receipt.and_then(|receipt| receipt.peak_rss_bytes).map_or("—".into(), bytes),
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
                Event::StepStarted { step } => format!("{step} started"),
                Event::StepFinished { step, reused: true, .. } => format!("{step} reused"),
                Event::StepFinished { step, receipt, .. } => {
                    format!("{step} built in {}, {}", duration(receipt.wall_ms), bytes(receipt.bytes_out))
                }
                Event::StepFailed { step, error } => format!("{step} failed: {error}"),
                Event::FetchStarted { source, version, params } => {
                    format!("{} fetch started", fetched(source, version, params))
                }
                Event::FetchFinished { source, version, params, bytes: size, wall_ms } => {
                    format!("{} fetched in {}, {}", fetched(source, version, params), duration(*wall_ms), bytes(*size))
                }
                Event::FetchFailed { source, version, params, error } => {
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

/// `SOURCE@VERSION NAME=VALUE…`
fn fetched(source: &str, version: &str, params: &[(String, String)]) -> String {
    let params = params.iter().map(|(name, value)| format!(" {name}={value}"));
    format!("{source}@{version}{}", params.collect::<String>())
}

fn mark(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Running => "◐",
        Outcome::Ok => "✓",
        Outcome::Failed => "✗",
    }
}

fn duration(ms: u64) -> String {
    let seconds = ms / 1000;
    match seconds {
        0 => format!("{ms} ms"),
        1..60 => format!("{seconds}s"),
        60..3600 => format!("{}m {:02}s", seconds / 60, seconds % 60),
        _ => format!("{}h {:02}m", seconds / 3600, seconds / 60 % 60),
    }
}

fn bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.1} kB", n as f64 / 1e3),
        1_000_000..1_000_000_000 => format!("{:.1} MB", n as f64 / 1e6),
        _ => format!("{:.2} GB", n as f64 / 1e9),
    }
}
