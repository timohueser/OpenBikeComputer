//! Local preparation uses the retained worker; known app serving owns a separate lock.

use std::path::Path;

use super::{operation_cli, Code, Error};
use crate::dev::{self, Prepared, Request};
use crate::engine::runs::{Event, Phase};
use crate::operation::{Kind, Status};
use crate::product::Product;
use crate::store::Store;
use clap::Args;

#[derive(Args)]
#[command(group(clap::ArgGroup::new("action").args(["prepare", "start", "stop", "open", "logs", "status"]).multiple(false)))]
pub(super) struct Dev {
    /// The saved region; defaults to the prior Local selection, then live.
    pub region: Option<String>,
    /// Take the current published versions instead of the saved Local versions.
    #[arg(long)]
    pub refresh_live: bool,
    #[arg(long)]
    pub prepare: bool,
    #[arg(long)]
    pub start: bool,
    #[arg(long)]
    pub stop: bool,
    #[arg(long)]
    pub open: bool,
    #[arg(long)]
    pub logs: bool,
    #[arg(long)]
    pub status: bool,
}

pub(super) fn run(root: &Path, args: Dev, json: bool) -> Result<(), Error> {
    let store = Store::open()?;
    if (args.start || args.stop || args.open || args.logs || args.status)
        && (args.region.is_some() || args.refresh_live)
    {
        return Err(Code::Usage.error("region and version refresh apply only to Local preparation"));
    }
    if args.stop {
        return super::print_json(&dev::Observed { state: dev::stop(&store)? });
    }
    if args.open {
        return dev::open(&store).map_err(Into::into);
    }
    if args.logs {
        let logs = dev::logs(&store)?;
        if json {
            return super::print_json(&dev::Logs { logs });
        }
        for line in logs {
            println!("{line}");
        }
        return Ok(());
    }
    if args.status {
        return super::print_json(&dev::Observed { state: dev::state(&store)? });
    }
    if !args.start {
        let handle = operation_cli::start(
            root,
            &store,
            crate::operation::Request {
                kind: Kind::DevPrepare,
                env: "local".into(),
                only: Vec::new(),
                moves: Vec::new(),
                plan: None,
                dev: Some(Request { region: args.region, refresh_live: args.refresh_live }),
            },
            None,
        )?;
        if args.prepare {
            return super::print_json(&handle);
        }
        if !json {
            eprintln!("Local preparation run {}. Stop or inspect it with obc data runs.", handle.run);
        }
        loop {
            let view = operation_cli::view(&store, &handle.run)?;
            match view.operation {
                Some(Status::Finished { ok: true }) => break,
                Some(Status::Finished { ok: false }) | Some(Status::Stopped | Status::Interrupted) => {
                    return Err(Code::RunFailed
                        .error("Local preparation did not finish; inspect the run")
                        .with_run(&handle.run));
                }
                _ => std::thread::sleep(std::time::Duration::from_millis(250)),
            }
        }
        return super::print_json(&dev::prepared(&store)?);
    }
    let prepared = dev::prepared(&store)?;
    let state = dev::start(root, &store, &prepared)?;
    if !json {
        dev::open(&store)?;
    }
    super::print_json(&dev::Observed { state: Some(state) })
}

pub(super) fn prepare(
    root: &Path,
    store: &Store,
    products: &[&dyn Product],
    id: &str,
    request: &Request,
) -> Result<(), Error> {
    let mut run = operation_cli::resume(store, "dev local")?
        .ok_or_else(|| Code::RunFailed.error("Local preparation has no retained operation").with_run(id))?;
    let result = (|| -> Result<Prepared, Error> {
        run.record(&Event::Phase { phase: Phase::Prepare })?;
        let product = products
            .iter()
            .find(|product| product.name() == "planner")
            .ok_or_else(|| Code::Blocked.error("this worker does not link the Local Web planner"))?;
        let prepared = product.dev_prepare(root, store, request, &mut run)?;
        run.check_stop(store)?;
        crate::commit::durable(
            &store.root().join("dev/local/prepared.json"),
            &serde_json::to_vec(&prepared).map_err(|e| e.to_string())?,
        )?;
        dev::replace(root, store, &prepared)?;
        Ok(prepared)
    })();
    let error = result.as_ref().err().map(|e| e.message.as_str());
    let finished = run.finish(error);
    let prepared = match result {
        Err(mut error) => {
            if let Err(message) = finished {
                error.message += &format!("; run journal: {message}");
            }
            return Err(error.with_run(id));
        }
        Ok(prepared) => {
            finished?;
            prepared
        }
    };
    super::print_json(&prepared)
}
