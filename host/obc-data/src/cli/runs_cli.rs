//! `obc data runs`: the runs in the store, the status of one run, its log as it comes, and a stop.

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use crate::engine::runs::{self, bytes, duration, fetched, RunStep, State, Status, Summary, View};
use crate::store::Store;

use super::{cells, print_json, print_table, Code, Error};

#[derive(Args)]
pub struct Runs {
    /// A run id: show its phases, layers and steps, and the end of its log.
    run: Option<String>,
    /// Write the log of RUN as it comes, until it ends; with `--json`, its status each time it
    /// changes. Ctrl-C ends only the following. The exit status is the one of the run.
    #[arg(long, requires = "run", conflicts_with = "stop")]
    follow: bool,
    /// Stop RUN before its next fetch, step or phase. It frees the lock of its target and keeps
    /// what it baked.
    #[arg(long, requires = "run")]
    stop: bool,
}

pub fn run(args: Runs, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    let Some(id) = args.run else {
        return list(&store, json);
    };
    let usage = |e: String| Code::Usage.error(e);
    runs::check_id(&id).map_err(usage)?;
    runs::status(&store, &id).map_err(usage)?;
    if args.stop {
        runs::stop(&store, &id).map_err(usage)?;
        if !json {
            println!("run {id} stops before its next fetch, step or phase");
            return Ok(());
        }
    }
    if args.follow {
        return follow(&store, &id, json);
    }
    show(&runs::view(&store, &id)?, json)
}

/// Every run in the store, newest first.
#[derive(Serialize, JsonSchema)]
pub struct RunList<'a> {
    runs: &'a [Summary],
}

fn list(store: &Store, json: bool) -> Result<(), Error> {
    let summaries = runs::list(store)?;
    let runs = summaries.as_slice();
    if json {
        return print_json(&RunList { runs });
    }
    let mut table = vec![cells(["RUN", "COMMAND", "", "TOOK", "FETCHED", "BUILT"])];
    for run in runs {
        table.push(vec![
            run.id.clone(),
            run.args.join(" "),
            mark(run.state).into(),
            run.wall_ms.map_or("—".into(), duration),
            bytes(run.bytes_fetched),
            bytes(run.bytes_built),
        ]);
    }
    print_table(&table);
    Ok(())
}

fn show(view: &View, json: bool) -> Result<(), Error> {
    if json {
        return print_json(view);
    }
    let run = &view.status;
    let took = run.wall_ms.map_or("—".into(), duration);
    println!("{}  obc data {}  {}  {took}", run.id, run.args.join(" "), mark(run.state));
    let phases: Vec<String> = run.phases.iter().map(|phase| format!("{} {}", phase.phase, mark(phase.state))).collect();
    if !phases.is_empty() {
        println!("phases: {}", phases.join(", "));
    }
    for layer in &run.layers {
        let mut text = format!("layer {}: {}/{}", layer.layer, layer.done, layer.total);
        if !layer.leaves_baking.is_empty() {
            text += &format!("; baking {}", layer.leaves_baking.join(", "));
        }
        println!("{text}");
    }
    if let Some(error) = &run.error {
        println!("{}\nfix: {}", error.message, error.fix);
        if !error.kept.is_empty() {
            println!("the store keeps the layers of {}", error.kept.join(", "));
        }
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
    let mut table = vec![cells(["STEP", "LAYER", "TOOK", "Δ LAST RUN", "PEAK RAM", "OUTPUT", "READS", "USED BY"])];
    for step in &run.steps {
        let [took, change, ram, output] = step_cells(step);
        table.push(vec![
            step.step.clone(),
            step.layer.clone(),
            took,
            change,
            ram,
            output,
            step.reads.join(", "),
            step.users.join(", "),
        ]);
    }
    if !run.steps.is_empty() {
        print_table(&table);
    }
    for line in &view.log {
        println!("  {line}");
    }
    Ok(())
}

fn follow(store: &Store, id: &str, json: bool) -> Result<(), Error> {
    runs::stop_on_signals();
    let ended = runs::follow(store, id, |lines, status| {
        match json {
            true => println!("{}", serde_json::to_string(status).map_err(|e| e.to_string())?),
            false => print!("{lines}"),
        }
        Ok(())
    })?;
    ended_as(id, ended)
}

/// Follow run `id` in the terminal that started it, until it ends: Ctrl-C ends only the following.
pub(super) fn follow_in_terminal(store: &Store, id: &str) -> Result<(), Error> {
    follow(store, id, false)
}

/// The result of a run that ended as `status`; `None` while it continues.
fn ended_as(id: &str, status: Option<Status>) -> Result<(), Error> {
    let Some(status) = status else {
        eprintln!("obc data: the run continues; `obc data runs {id} --stop` stops it");
        return Ok(());
    };
    let Some(error) = status.error.filter(|_| status.state != State::Ok) else { return Ok(()) };
    let code = serde_json::from_value(serde_json::Value::String(error.code)).unwrap_or(Code::RunFailed);
    let mut failed = Code::error(code, error.message).fix(error.fix);
    failed.run = Some(id.into());
    Err(failed)
}

/// The time of a step, its change since the last run that built it, its peak RAM and its output.
fn step_cells(step: &RunStep) -> [String; 4] {
    let took = match step.wall_ms {
        Some(_) if step.reused => "reused".into(),
        Some(wall_ms) => duration(wall_ms),
        None if step.error.is_some() => "—".into(),
        None => "running".into(),
    };
    let change = match (step.wall_ms.filter(|_| !step.reused), step.last_wall_ms) {
        (Some(took), Some(last)) if took == last => "=".into(),
        (Some(took), Some(last)) if took > last => format!("+{}", duration(took - last)),
        (Some(took), Some(last)) => format!("−{}", duration(last - took)),
        _ => "—".into(),
    };
    let ram = step.peak_rss_bytes.map_or("—".into(), bytes);
    let output = match (&step.error, step.bytes_out) {
        (Some(error), _) => format!("✗ {error}"),
        (None, Some(size)) => bytes(size),
        (None, None) => "—".into(),
    };
    [took, change, ram, output]
}

fn mark(state: State) -> &'static str {
    match state {
        State::Running => "◐",
        State::Ok => "✓",
        State::Failed => "✗",
        State::Stopped => "■",
    }
}
