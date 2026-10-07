//! The shared start API retains the checked worker before the viewing process can exit.

use std::fs::File;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::runs::{self, Run};
use crate::engine::LayerFile;
use crate::operation::{self, Control, Kind, Request, State};
use crate::store::{hash_file, Store};

use super::{api::Error, build_cli::EnvPlan, Code};

mod observe;
pub(crate) use observe::tail;
pub use observe::{view, View};

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Handle {
    pub run: String,
    pub request: String,
}

struct Session {
    store: Store,
    control: Control,
}

static SESSION: OnceLock<Session> = OnceLock::new();

fn file(path: &Path, name: &str) -> Result<LayerFile, String> {
    let (sha256, size) = hash_file(path)?;
    Ok(LayerFile { path: name.into(), size, sha256 })
}

/// The caller reviews any apply plan before invoking this API. Nothing waits for another run.
pub fn start(root: &Path, store: &Store, mut request: Request, plan: Option<&EnvPlan>) -> Result<Handle, Error> {
    if SESSION.get().is_some() {
        return Err(Code::Usage.error("an operation worker cannot start a second operation"));
    }
    let code = crate::worker::bound_code(root).map_err(|e| Code::Blocked.error(e))?;
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let plan_bytes = plan
        .map(|plan| {
            super::build_cli::complete(Some(plan))?;
            if plan.env != request.env {
                return Err(Code::Usage.error("the saved plan names another environment"));
            }
            serde_json::to_vec(plan).map_err(|e| Code::InvalidData.error(e.to_string()))
        })
        .transpose()?;
    request.plan = plan_bytes.as_ref().map(|bytes| LayerFile {
        path: "plan.json".into(),
        size: bytes.len() as u64,
        sha256: crate::store::sha256_hex(bytes),
    });
    request.check().map_err(|e| Code::Usage.error(e))?;
    let command = command(&request);
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    if root.to_str().is_none() || store.root().to_str().is_none() {
        return Err(Code::Usage.error("operation root and store paths must be UTF-8"));
    }
    let worker_identity = file(&executable, "worker")?;
    let request_sha256 = request.digest()?;
    let run = Run::create(store, &command)?;
    let normalized = Store::at(store.root().canonicalize().map_err(|e| e.to_string())?);
    let store = &normalized;
    let id = run.id().to_string();
    let directory = operation::directory(store, &id)?;
    let worker = directory.join("worker");
    let control = Control {
        run: id.clone(),
        request_sha256,
        request,
        root,
        worker: worker_identity,
        code,
        state: State::Reserved,
    };
    let mut reserved = false;
    let initialized = (|| -> Result<(), Error> {
        if let operation::Reservation::Busy(reason) = operation::reserve(store, &control)? {
            return Err(Code::Busy.error(reason));
        }
        reserved = true;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        }
        if let Some(bytes) = &plan_bytes {
            crate::store::durable(&directory.join("plan.json"), bytes)?;
        }
        std::fs::copy(&executable, &worker).map_err(|e| e.to_string())?;
        File::open(&worker).and_then(|file| file.sync_all()).map_err(|e| e.to_string())?;
        crate::store::durable_directory(&directory)?;
        if file(&worker, "worker")? != control.worker {
            return Err(Code::Blocked.error("producer executable changed during retention"));
        }
        if operation::read(store, &id)?.is_none_or(|control| control.state != State::Reserved) {
            return Err(Code::Blocked.error("operation stopped before worker launch"));
        }
        run.sync()?;
        Ok(())
    })();
    if let Err(mut error) = initialized {
        if reserved {
            if let Err(message) = operation::stop(store, &id) {
                error.message += &format!("; stop could not persist: {message}");
            }
            remove_worker(store, &id);
        }
        if let Err(message) = run.finish(Some(&error.message)) {
            error.message += &format!("; run journal could not finish: {message}");
            if error.code == Code::Busy {
                error.code = Code::Failed;
                error.fix = Code::Failed.fix().into();
            }
        }
        return Err(error.with_run(&id));
    }
    let handle = Handle { run: id.clone(), request: control.request_sha256.clone() };
    drop(run);
    let mut child = Command::new(&worker);
    child
        .args([
            "--json",
            "perform",
            "--store",
            store.root().to_str().ok_or_else(|| Code::Usage.error("store path is not UTF-8"))?,
            "--run",
            &id,
            "--request",
            &handle.request,
        ])
        .current_dir(&control.root)
        .env(crate::worker::ROOT, &control.root)
        .env(crate::worker::CODE, &control.code)
        .env(crate::worker::EXE, &control.worker.sha256)
        .env("OBC_DATA_STORE", store.root());
    if let Err(message) = operation::launch::detach(&mut child, &directory) {
        return Err(unlaunched(store, &id, Code::Failed.error(message)));
    }
    let mut spawned = match child.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Err(unlaunched(store, &id, Code::Failed.error(format!("detached launch failed: {error}"))));
        }
    };
    std::thread::spawn(move || {
        let _ = spawned.wait();
    });
    Ok(handle)
}

fn command(request: &Request) -> String {
    let kind = match request.kind {
        Kind::Prepare => "prepare",
        Kind::Build => "build",
        Kind::Apply => "apply",
        Kind::DevPrepare => "dev",
    };
    format!("{kind} {}", request.env)
}

fn unlaunched(store: &Store, run: &str, mut error: Error) -> Error {
    if let Err(message) = operation::stop(store, run) {
        error.message += &format!("; stop could not persist: {message}");
    }
    let finished = (|| -> Result<(), String> {
        Run::attach(store, run, &runs::events(store, run)?)?.finish(Some(&error.message))
    })();
    if let Err(message) = finished {
        error.message += &format!("; run journal could not finish: {message}");
    }
    remove_worker(store, run);
    error.with_run(run)
}

/// A private child can only execute its immutable request through the retained checked binary.
pub(super) fn enter(store: &Store, run: &str, request: &str) -> Result<crate::store::Lock, Error> {
    let control = operation::read(store, run)?.ok_or_else(|| Code::Usage.error("run has no detached request"))?;
    if control.root != super::root()?.canonicalize().map_err(|e| e.to_string())?
        || file(&std::env::current_exe().map_err(|e| e.to_string())?, "worker")? != control.worker
    {
        return Err(Code::Blocked.error("detached operation belongs to another checkout or worker"));
    }
    if let Some(plan) = &control.request.plan {
        if file(&operation::directory(store, run)?.join("plan.json"), "plan.json")? != *plan {
            return Err(Code::PlanOutdated.error("detached saved plan bytes changed"));
        }
    }
    let using = operation::claim(store, run, request)?;
    std::env::set_var("OBC_DATA_STORE", store.root());
    let (watched, id) = (Store::at(store.root()), run.to_string());
    std::thread::spawn(move || loop {
        if operation::stopped(&watched, &id).unwrap_or(false) {
            return crate::fetch::http::stop();
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    });
    SESSION
        .set(Session { store: Store::at(store.root()), control })
        .map_err(|_| Code::Usage.error("this worker already owns an operation"))?;
    Ok(using)
}

pub(super) fn resume(store: &Store, command: &str) -> Result<Option<Run>, String> {
    let Some(session) = SESSION.get() else { return Ok(None) };
    if session.store.root() != store.root() || command != self::command(&session.control.request) {
        return Err("operation child cannot start another run or environment".into());
    }
    Ok(Some(Run::attach(store, &session.control.run, &runs::events(store, &session.control.run)?)?))
}

fn finish_result(store: &Store, run: &str, mut result: Result<(), Error>) -> Result<(), Error> {
    if let Err(error) = &mut result {
        error.run = Some(run.into());
    }
    let finished = (|| -> Result<(), String> {
        let control = operation::read(store, run)?.ok_or("operation has no control")?;
        let sealed = if control.state == State::Running {
            use std::io::Write;
            let directory = operation::directory(store, run)?;
            let bytes = match &result {
                Err(error) => serde_json::to_vec(&super::api::Failure { error }).map_err(|e| e.to_string())?,
                Ok(()) => {
                    std::io::stdout().flush().map_err(|e| e.to_string())?;
                    let bytes = std::fs::read(directory.join("stdout.json")).map_err(|e| e.to_string())?;
                    serde_json::from_slice::<serde_json::Value>(&bytes).map_err(|e| e.to_string())?;
                    bytes
                }
            };
            let path = directory.join("result.json");
            crate::store::durable(&path, &bytes)?;
            Some(file(&path, "result.json")?)
        } else {
            None
        };
        operation::finish(store, run, result.is_ok(), sealed)
    })();
    if let Err(message) = finished {
        match &mut result {
            Err(error) => error.message += &format!("; operation result could not finish: {message}"),
            Ok(()) => {
                result = Err(Code::Failed.error(format!("operation result could not finish: {message}")).with_run(run))
            }
        }
    }
    result
}

pub(super) fn plan_path() -> Result<Option<PathBuf>, String> {
    let session = SESSION.get().ok_or("worker has no operation request")?;
    session
        .control
        .request
        .plan
        .as_ref()
        .map(|_| {
            operation::directory(&session.store, &session.control.run).map(|directory| directory.join("plan.json"))
        })
        .transpose()
}

pub(super) fn request() -> Result<&'static Request, String> {
    SESSION.get().map(|session| &session.control.request).ok_or_else(|| "worker has no operation request".into())
}

pub(super) fn perform(
    store: &Store,
    run: &str,
    digest: &str,
    products: &[&dyn crate::product::Product],
) -> Result<(), Error> {
    let _using = enter(store, run, digest)?;
    let request = request()?;
    let root = super::root()?;
    let result = match request.kind {
        Kind::DevPrepare => {
            super::dev_cli::prepare(&root, store, products, run, request.dev.as_ref().expect("checked Local request"))
        }
        Kind::Prepare => super::build_cli::prepare(
            &root,
            products,
            super::build_cli::PlanArgs {
                env: request.env.clone(),
                only: request.only.clone(),
                moves: request.moves.clone(),
            },
            true,
        ),
        Kind::Build => super::build_cli::build(
            &root,
            products,
            super::build_cli::BuildArgs {
                env: request.env.clone(),
                only: request.only.clone(),
                moves: request.moves.clone(),
                plan: plan_path()?,
            },
            true,
        ),
        Kind::Apply => super::apply_cli::apply(
            &root,
            products,
            super::apply_cli::ApplyArgs { env: request.env.clone(), plan: plan_path()?, yes: true },
            true,
        ),
    };
    let result = finish_result(store, run, result);
    // Every admitted producer has drained before the binary goes.
    if operation::read(store, run)?.is_some_and(|control| control.state.terminal()) {
        remove_worker(store, run);
    }
    result
}

fn remove_worker(store: &Store, run: &str) {
    let result =
        operation::directory(store, run).and_then(|directory| match std::fs::remove_file(directory.join("worker")) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.to_string()),
        });
    if let Err(error) = result {
        eprintln!("retained worker cleanup: {error}");
    }
}

pub(super) fn print_handle(handle: &Handle, json: bool) -> Result<(), Error> {
    if json {
        return super::print_json(handle);
    }
    println!(
        "Started run {}. Inspect with `obc data runs {}`; stop with `obc data runs {} --stop`.",
        handle.run, handle.run, handle.run
    );
    Ok(())
}

pub(super) fn prepare(root: &Path, args: super::build_cli::PlanArgs, json: bool) -> Result<(), Error> {
    let request =
        Request { kind: Kind::Prepare, env: args.env, only: args.only, moves: args.moves, plan: None, dev: None };
    print_handle(&start(root, &Store::open()?, request, None)?, json)
}

pub(super) fn build(root: &Path, args: super::build_cli::BuildArgs, json: bool) -> Result<(), Error> {
    let plan = args.plan.as_deref().map(super::build_cli::read_plan).transpose()?;
    let request =
        Request { kind: Kind::Build, env: args.env, only: args.only, moves: args.moves, plan: None, dev: None };
    print_handle(&start(root, &Store::open()?, request, plan.as_ref())?, json)
}

pub(super) fn apply(
    root: &Path,
    products: &[&dyn crate::product::Product],
    args: super::apply_cli::ApplyArgs,
    json: bool,
) -> Result<(), Error> {
    let consent = super::apply_cli::consent(&args, std::io::stdin().is_terminal())?;
    if args.env != "live" {
        return Err(Code::Usage.error("only live applies"));
    }
    let store = Store::open()?;
    let plan = match args.plan.as_deref() {
        Some(path) => super::build_cli::read_plan(path)?,
        None => super::build_cli::plan_live(
            root,
            &store,
            &crate::fetch::http::Http::new(),
            &super::remote()?,
            products,
            &[],
            false,
        )?,
    };
    super::build_cli::complete(Some(&plan))?;
    if !json {
        super::build_cli::print_plan(&plan);
    }
    super::api::confirm(&super::apply_cli::question(&plan), consent)?;
    let request =
        Request { kind: Kind::Apply, env: args.env, only: Vec::new(), moves: Vec::new(), plan: None, dev: None };
    print_handle(&start(root, &store, request, Some(&plan))?, json)
}
