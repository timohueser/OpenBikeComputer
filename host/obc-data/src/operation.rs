//! Detached preparation and build ownership, followed by an irreversible publication handoff.

use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::runs::check_id;
use crate::engine::LayerFile;
use crate::store::{sha256_hex, Store};

pub mod budget;
pub mod launch;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Prepare,
    Build,
    Apply,
    Auto,
    DevPrepare,
}

/// The saved plan and worker live in the operation's private directory.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub kind: Kind,
    pub env: String,
    pub only: Vec<String>,
    pub moves: Vec<String>,
    pub plan: Option<LayerFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dev: Option<crate::dev::Request>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fixture: Option<Box<crate::fixtures::Plan>>,
}

impl Request {
    pub fn check(&self) -> Result<(), String> {
        if !crate::is_kebab(&self.env) {
            return Err("operation environment is not a normalized name".into());
        }
        if self.env == "fixtures"
            && self.fixture.is_none()
            && (self.kind != Kind::Prepare
                || self.dev.is_some()
                || self.plan.is_some()
                || !self.moves.is_empty()
                || self.only.iter().any(|id| !crate::is_kebab(id)))
        {
            return Err("fixture operations need exact package preparation or a typed reviewed apply plan".into());
        }
        if self.fixture.is_some() {
            if self.kind != Kind::Apply
                || self.env != "fixtures"
                || self.dev.is_some()
                || self.plan.is_some()
                || !self.only.is_empty()
                || !self.moves.is_empty()
            {
                return Err("fixture apply takes only its reviewed collection plan".into());
            }
            return self.fixture.as_ref().expect("present above").check();
        }
        if (self.kind == Kind::DevPrepare) != self.dev.is_some()
            || self.dev.is_some()
                && (self.env != "local" || self.plan.is_some() || !self.only.is_empty() || !self.moves.is_empty())
        {
            return Err("Local preparation takes only its explicit region and source".into());
        }
        if self.kind == Kind::Apply
            && (self.env != "live" || self.plan.is_none() || !self.only.is_empty() || !self.moves.is_empty())
        {
            return Err("apply takes only the reviewed plan of live".into());
        }
        if self.kind == Kind::Auto
            && (self.env == "fixture" || self.env == "fixtures" || self.env.starts_with("fixture-"))
        {
            return Err("fixture environments do not support automation".into());
        }
        if self.kind == Kind::Auto && (self.plan.is_some() || !self.only.is_empty() || !self.moves.is_empty()) {
            return Err("auto owns its used stale requests and takes no saved plan or selections".into());
        }
        if self.kind == Kind::Prepare && self.plan.is_some() {
            return Err("prepare resolves inputs before a saved plan exists".into());
        }
        if self.plan.is_some() && (!self.only.is_empty() || !self.moves.is_empty()) {
            return Err("a saved plan already owns its selections and source moves".into());
        }
        if self.plan.as_ref().is_some_and(|plan| plan.path != "plan.json" || !digest(&plan.sha256)) {
            return Err("operation saved plan is not a pinned plan.json".into());
        }
        Ok(())
    }

    pub fn digest(&self) -> Result<String, String> {
        self.check()?;
        Ok(sha256_hex(&serde_json::to_vec(self).map_err(|e| e.to_string())?))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum State {
    Reserved,
    Running,
    Stopping,
    Stopped,
    /// A dispatch can still arrive after its initiating transport is lost.
    Owner {
        host: String,
        bundle: String,
    },
    Finished {
        ok: bool,
        result: LayerFile,
    },
    Resolved {
        host: String,
        bundle: String,
        ok: bool,
        result: LayerFile,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Status {
    Starting,
    Running,
    Stopping,
    Stopped,
    Interrupted,
    /// Remote absence or a transport error does not resolve a dispatched operation.
    AwaitingOwner {
        host: String,
        bundle: String,
    },
    Finished {
        ok: bool,
    },
    UnknownOwner {
        host: String,
        bundle: String,
        reason: String,
    },
}

/// This observation never creates locks, edits control, or rewrites the original journal.
pub fn status(store: &Store, run: &str) -> Result<Option<Status>, String> {
    let Some(control) = read(store, run)? else { return Ok(None) };
    let active = || store.is_locked(&format!("operation-active-{}", control.request.env));
    Ok(Some(match control.state {
        State::Reserved => Status::Starting,
        State::Running if active()? => Status::Running,
        State::Running => Status::Interrupted,
        State::Stopping if active()? => Status::Stopping,
        State::Stopping => Status::Stopped,
        State::Stopped => Status::Stopped,
        State::Owner { host, bundle } => Status::AwaitingOwner { host, bundle },
        State::Finished { ok, .. } | State::Resolved { ok, .. } => Status::Finished { ok },
    }))
}

impl State {
    pub(crate) fn terminal(&self) -> bool {
        matches!(self, Self::Stopped | Self::Finished { .. } | Self::Resolved { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Control {
    pub run: String,
    pub request: Request,
    pub request_sha256: String,
    pub root: PathBuf,
    pub worker: LayerFile,
    pub code: String,
    pub state: State,
}

pub fn directory(store: &Store, run: &str) -> Result<PathBuf, String> {
    check_id(run)?;
    Ok(store.root().join("operations").join(run))
}

fn path(store: &Store, run: &str) -> Result<PathBuf, String> {
    Ok(directory(store, run)?.join("control.json"))
}

/// A missing control is distinct from a corrupt or unreadable operation.
pub fn read(store: &Store, run: &str) -> Result<Option<Control>, String> {
    let bytes = match std::fs::read(path(store, run)?) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let control: Control = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    if control.run != run
        || control.request.digest()? != control.request_sha256
        || control.worker.path != "worker"
        || !digest(&control.worker.sha256)
        || !digest(&control.code)
        || matches!(&control.state, State::Finished { result, .. } if result.path != "result.json" || !digest(&result.sha256))
        || matches!(&control.state, State::Resolved { result, .. } if result.path != "owner-result.json" || !digest(&result.sha256))
    {
        return Err("operation control differs from its run or immutable request".into());
    }
    Ok(Some(control))
}

fn save(store: &Store, control: &Control) -> Result<(), String> {
    crate::commit::durable(&path(store, &control.run)?, &serde_json::to_vec(control).map_err(|e| e.to_string())?)
}

pub(crate) fn active_path(store: &Store, env: &str) -> PathBuf {
    store.root().join("operations").join(format!("{env}.active"))
}

#[derive(Debug, PartialEq, Eq)]
pub enum Reservation {
    Reserved,
    Busy(String),
}

/// Reserve one environment without a waiting queue or an expiry that can admit an old child.
pub fn reserve(store: &Store, control: &Control) -> Result<Reservation, String> {
    control.request.check()?;
    check_id(&control.run)?;
    if control.state != State::Reserved || control.request.digest()? != control.request_sha256 {
        return Err("a new operation needs its exact reserved request".into());
    }
    let _lock = store.lock(&format!("operation-control-{}", control.request.env))?;
    if store.is_locked(&format!("operation-active-{}", control.request.env))? {
        return Ok(Reservation::Busy("the environment worker has not drained yet".into()));
    }
    let active = active_path(store, &control.request.env);
    match std::fs::read_to_string(&active) {
        Ok(id) => {
            let previous = read(store, &id)?.ok_or("active operation has no control")?;
            if !previous.state.terminal() {
                return Ok(Reservation::Busy(format!(
                    "operation {} already owns {}; inspect or stop it",
                    id, control.request.env
                )));
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.to_string()),
    }
    if read(store, &control.run)?.is_some() {
        return Err("an operation id cannot be rebound to another request".into());
    }
    save(store, control)?;
    crate::commit::durable(&active, control.run.as_bytes())?;
    Ok(Reservation::Reserved)
}

fn change<T>(store: &Store, run: &str, update: impl FnOnce(&mut Control) -> Result<T, String>) -> Result<T, String> {
    let before = read(store, run)?.ok_or("run has no detached operation")?;
    let _lock = store.lock(&format!("operation-control-{}", before.request.env))?;
    let mut control = read(store, run)?.ok_or("run has no detached operation")?;
    if control.request_sha256 != before.request_sha256 {
        return Err("operation request changed while taking its control lock".into());
    }
    let result = update(&mut control)?;
    save(store, &control)?;
    Ok(result)
}

pub fn stop(store: &Store, run: &str) -> Result<State, String> {
    change(store, run, |control| {
        control.state = match &control.state {
            State::Reserved => State::Stopped,
            State::Running | State::Stopping => {
                if store.is_locked(&format!("operation-active-{}", control.request.env))? {
                    State::Stopping
                } else {
                    State::Stopped
                }
            }
            State::Owner { .. } => {
                return Err("publication has started; it cannot stop safely; reconcile the owner result".into())
            }
            other => other.clone(),
        };
        Ok(control.state.clone())
    })
}

pub fn stopped(store: &Store, run: &str) -> Result<bool, String> {
    Ok(read(store, run)?.is_some_and(|control| matches!(control.state, State::Stopping | State::Stopped)))
}

/// Only the reserved child takes the long environment lock. Stop and claim use the same short lock.
pub fn claim(store: &Store, run: &str, request: &str) -> Result<crate::store::Lock, String> {
    let control = read(store, run)?.ok_or("run has no detached operation")?;
    let using = store
        .try_lock(&format!("operation-active-{}", control.request.env))?
        .ok_or("another worker already owns the environment")?;
    change(store, run, |control| {
        if control.request_sha256 != request
            || control.state != State::Reserved
            || std::fs::read_to_string(active_path(store, &control.request.env)).map_err(|e| e.to_string())? != run
        {
            return Err("the detached worker no longer owns the reserved request".into());
        }
        control.state = State::Running;
        Ok(())
    })?;
    Ok(using)
}

/// Local completion cannot resolve a handed-off publication outcome.
pub fn finish(store: &Store, run: &str, ok: bool, result: Option<LayerFile>) -> Result<(), String> {
    change(store, run, |control| {
        control.state = match &control.state {
            State::Running => {
                let result = result.ok_or("completed operation has no sealed result")?;
                if result.path != "result.json" || !digest(&result.sha256) {
                    return Err("completed operation result is not a pinned result.json".into());
                }
                State::Finished { ok, result }
            }
            State::Stopping => State::Stopped,
            State::Stopped => State::Stopped,
            State::Finished { .. } | State::Resolved { .. } => return Ok(()),
            State::Owner { .. } => return Ok(()),
            _ => return Err("operation has no running worker to finish".into()),
        };
        Ok(())
    })
}

/// Only a checked owner response may durably resolve the local handoff.
pub fn resolve(store: &Store, run: &str, host: &str, bundle: &str, ok: bool, result: LayerFile) -> Result<(), String> {
    if result.path != "owner-result.json" || !digest(&result.sha256) {
        return Err("owner result is not a pinned owner-result.json".into());
    }
    change(store, run, |control| {
        if control.state != (State::Owner { host: host.into(), bundle: bundle.into() }) {
            return Err("owner result names another run or bundle".into());
        }
        control.state = State::Resolved { host: host.into(), bundle: bundle.into(), ok, result };
        Ok(())
    })
}

pub fn handoff(store: &Store, run: &str, host: &str, bundle: &str) -> Result<(), String> {
    if !digest(bundle) || host.is_empty() {
        return Err("publication handoff needs its exact owner and bundle".into());
    }
    change(store, run, |control| {
        if control.state != State::Running {
            return Err("operation stopped before publication handoff".into());
        }
        if !matches!(control.request.kind, Kind::Apply | Kind::Auto) || control.request.env != "live" {
            return Err("only live apply or auto can hand off publication".into());
        }
        control.state = State::Owner { host: host.into(), bundle: bundle.into() };
        Ok(())
    })
}

fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    fn control(run: &str) -> Control {
        let request = Request {
            kind: Kind::Build,
            env: "local".into(),
            only: Vec::new(),
            moves: Vec::new(),
            plan: None,
            dev: None,
            fixture: None,
        };
        Control {
            run: run.into(),
            request_sha256: request.digest().unwrap(),
            request,
            root: "/checkout".into(),
            worker: LayerFile { path: "worker".into(), size: 1, sha256: "a".repeat(64) },
            code: "b".repeat(64),
            state: State::Reserved,
        }
    }

    #[test]
    fn stop_invalidates_a_delayed_child_and_an_environment_has_no_waiting_queue() {
        let temporary = Scratch::new("operation-reservation");
        let store = Store::at(temporary.0.clone());
        let first = control("2026-10-06-120000");
        let next = control("2026-10-06-120001");
        reserve(&store, &first).unwrap();
        assert!(
            matches!(reserve(&store, &next).unwrap(), Reservation::Busy(reason) if reason.contains("already owns"))
        );
        assert!(read(&store, &next.run).unwrap().is_none(), "busy has no admitted control");
        assert_eq!(stop(&store, &first.run).unwrap(), State::Stopped);
        reserve(&store, &next).unwrap();
        assert!(claim(&store, &first.run, &first.request_sha256).is_err());
        let using = claim(&store, &next.run, &next.request_sha256).unwrap();
        assert!(
            matches!(reserve(&store, &first).unwrap(), Reservation::Busy(reason) if reason.contains("not drained"))
        );
        assert_eq!(stop(&store, &next.run).unwrap(), State::Stopping);
        assert!(stopped(&store, &next.run).unwrap());
        finish(&store, &next.run, false, None).unwrap();
        drop(using);
        assert_eq!(read(&store, &next.run).unwrap().unwrap().state, State::Stopped);
    }

    #[test]
    fn stop_drains_a_started_producer_and_prevents_the_next_step() {
        use crate::engine::{
            tests::{fixture, step, steps_crate},
            Request as StepRequest, Run as Producer,
        };
        fn stop_after_output(request: &StepRequest) -> Result<(), String> {
            std::fs::write(request.output.join("first.txt"), "finished current work").map_err(|e| e.to_string())?;
            stop(
                &Store::at(request.options["store"].as_str().ok_or("test store")?),
                request.options["run"].as_str().ok_or("test run")?,
            )?;
            Ok(())
        }
        fn next(request: &StepRequest) -> Result<(), String> {
            std::fs::write(request.output.join("next.txt"), "not admitted").map_err(|e| e.to_string())
        }
        let fixture = fixture("operation-drain");
        let mut run = crate::engine::runs::Run::create(&fixture.store, "build local").unwrap();
        let id = run.id().to_string();
        let first = control(&id);
        reserve(&fixture.store, &first).unwrap();
        let using = claim(&fixture.store, &id, &first.request_sha256).unwrap();
        let mut steps = vec![
            step("test/first", Vec::new(), steps_crate(), "first.txt", Producer::Rust(stop_after_output)),
            step("test/next", Vec::new(), steps_crate(), "next.txt", Producer::Rust(next)),
        ];
        steps[0].options = serde_json::json!({"store":fixture.store.root(), "run":id});
        let plan = fixture.plan(&steps).unwrap();
        let root = fixture.root();
        let http = crate::fetch::http::Http::new();
        let context = crate::engine::runs::Context {
            store: &fixture.store,
            root: &root,
            sources: &[],
            http: &http,
            copies: None,
            limits: crate::engine::runs::Limits { jobs: 2, memory_bytes: None },
        };
        let error = run.build(&context, &steps, &plan).unwrap_err();
        assert!(error.contains("stopped after the current work"), "{error}");
        let receipts = fixture.store.layers().unwrap();
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].step, "test/first");
        assert_eq!(
            std::fs::read(fixture.store.object(&receipts[0].files[0].sha256)).unwrap(),
            b"finished current work"
        );
        assert!(handoff(&fixture.store, &id, "publisher", &"c".repeat(64)).is_err());
        run.finish(Some(&error)).unwrap();
        finish(&fixture.store, &id, false, None).unwrap();
        drop(using);
        assert_eq!(status(&fixture.store, &id).unwrap(), Some(Status::Stopped));
    }

    #[test]
    fn handoff_is_bound_and_stays_irreversible_after_the_local_worker_exits() {
        let temporary = Scratch::new("operation-handoff");
        let store = Store::at(temporary.0.clone());
        let mut first = control("2026-10-06-120000");
        first.request.kind = Kind::Apply;
        first.request.env = "live".into();
        first.request.plan = Some(LayerFile { path: "plan.json".into(), size: 1, sha256: "d".repeat(64) });
        first.request_sha256 = first.request.digest().unwrap();
        reserve(&store, &first).unwrap();
        assert!(claim(&store, &first.run, &"b".repeat(64)).is_err());
        let using = claim(&store, &first.run, &first.request_sha256).unwrap();
        handoff(&store, &first.run, "publisher", &"c".repeat(64)).unwrap();
        drop(using);
        assert!(stop(&store, &first.run).unwrap_err().contains("cannot stop safely"));
        finish(&store, &first.run, false, None).unwrap();
        assert_eq!(
            read(&store, &first.run).unwrap().unwrap().state,
            State::Owner { host: "publisher".into(), bundle: "c".repeat(64) }
        );
        let owner = read(&store, &first.run).unwrap().unwrap();
        assert!(matches!(
            reserve(&store, &first).unwrap(),
            Reservation::Busy(reason) if reason.contains("already owns live")
        ));
        assert_eq!(read(&store, &first.run).unwrap().unwrap(), owner);
        let mut local = control("2026-10-06-120001");
        local.request.kind = Kind::Auto;
        local.request_sha256 = local.request.digest().unwrap();
        reserve(&store, &local).unwrap();
        let local_using = claim(&store, &local.run, &local.request_sha256).unwrap();
        assert!(handoff(&store, &local.run, "publisher", &"e".repeat(64)).unwrap_err().contains("only live"));
        assert_eq!(read(&store, &local.run).unwrap().unwrap().state, State::Running);
        assert_eq!(stop(&store, &local.run).unwrap(), State::Stopping);
        finish(&store, &local.run, false, None).unwrap();
        drop(local_using);
        assert_eq!(read(&store, &local.run).unwrap().unwrap().state, State::Stopped);
    }
}
