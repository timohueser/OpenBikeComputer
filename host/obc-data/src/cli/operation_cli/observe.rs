//! Read-only owner results and explicit local reconciliation.

use schemars::JsonSchema;
use serde::Serialize;

#[cfg(not(test))]
use super::remove_worker;
use crate::cli::{Code, Error};
use crate::engine::runs::{self, Run};
use crate::operation::{self, State};
use crate::store::Store;

#[derive(Debug, Serialize, JsonSchema)]
pub struct View {
    pub run: runs::Details,
    pub operation: Option<operation::Status>,
    pub observation_error: Option<String>,
    pub result: Option<serde_json::Value>,
    /// Recent worker stderr. Reading it does not change the run or owner state.
    pub logs: Vec<String>,
}

/// Observation computes the owner outcome without changing the control or local journal.
pub fn view(store: &Store, run: &str) -> Result<View, Error> {
    let mut view = local(store, run)?;
    if let Some(operation::Status::AwaitingOwner { host, bundle }) = &view.operation {
        #[cfg(not(test))]
        match crate::cli::commit_cli::lifetime::query(host, run, bundle) {
            Ok(observed) => apply_observation(store, &mut view, &observed)?,
            Err(error) => {
                view.observation_error = Some(match view.observation_error.take() {
                    Some(previous) => format!("{previous}; owner: {error}"),
                    None => error,
                })
            }
        }
        #[cfg(test)]
        let _ = (host, bundle);
    } else if matches!(view.operation, Some(operation::Status::Finished { .. })) {
        view.result = output(store, run)?;
    }
    if matches!(
        view.operation,
        Some(
            operation::Status::Starting
                | operation::Status::Running
                | operation::Status::Stopping
                | operation::Status::AwaitingOwner { .. }
                | operation::Status::UnknownOwner { .. }
        )
    ) {
        view.run.summary.outcome = runs::Outcome::Running;
    }
    Ok(view)
}

fn local(store: &Store, run: &str) -> Result<View, Error> {
    let mut view = View {
        run: runs::details(store, run)?,
        operation: operation::status(store, run)?,
        observation_error: None,
        result: None,
        logs: Vec::new(),
    };
    match tail(&operation::directory(store, run)?.join("stderr.log")) {
        Ok(lines) => view.logs = lines,
        Err(error) => view.observation_error = Some(format!("worker log: {error}")),
    }
    Ok(view)
}

pub(crate) fn tail(path: &std::path::Path) -> Result<Vec<String>, String> {
    use std::io::{Read, Seek, SeekFrom};
    const BYTES: u64 = 16 * 1024;
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    if !metadata.is_file() {
        return Err("stderr.log is not a regular file".into());
    }
    let start = metadata.len().saturating_sub(BYTES);
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(start)).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(BYTES).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if start > 0 { text.split_once('\n').map_or(text.as_ref(), |(_, rest)| rest) } else { text.as_ref() };
    let mut lines: Vec<_> = text.lines().rev().take(12).map(String::from).collect();
    lines.reverse();
    Ok(lines)
}

fn output(store: &Store, run: &str) -> Result<Option<serde_json::Value>, Error> {
    let directory = operation::directory(store, run)?;
    if let Some(control) = operation::read(store, run)? {
        if let State::Resolved { ref result, ok, .. } | State::Finished { ref result, ok } = control.state {
            let path = directory.join(&result.path);
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            if bytes.len() as u64 != result.size || crate::store::sha256_hex(&bytes) != result.sha256 {
                return Err(Code::VerifyFailed.error("sealed operation result bytes changed"));
            }
            let value: serde_json::Value =
                serde_json::from_slice(&bytes).map_err(|e| Code::Failed.error(e.to_string()))?;
            if matches!(control.state, State::Resolved { .. }) && value["status"] != if ok { "done" } else { "failed" }
            {
                return Err(Code::VerifyFailed.error("sealed owner result differs from its recorded outcome"));
            }
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn apply_observation(
    store: &Store,
    view: &mut View,
    observed: &crate::cli::commit_cli::lifetime::Observation,
) -> Result<(), Error> {
    let Some(operation::Status::AwaitingOwner { host, bundle }) = &view.operation else {
        return Err(Code::Usage.error("this run has no unresolved owner handoff"));
    };
    if observed.run != view.run.summary.id
        || observed.bundle != *bundle
        || observed.state.as_ref().is_some_and(|state| state.run != observed.run || state.bundle != *bundle)
    {
        return Err(Code::VerifyFailed.error("owner observation names another run or bundle"));
    }
    view.result = None;
    let Some(state) = &observed.state else { return Ok(()) };
    if let Some(pending) = &state.pending {
        if observed.active && observed.reply.is_none() {
            return Ok(());
        }
        view.result = None;
        view.operation = Some(operation::Status::UnknownOwner {
            host: host.clone(),
            bundle: bundle.clone(),
            reason: serde_json::to_string(&pending.mutation).map_err(|e| e.to_string())?,
        });
        return Ok(());
    }
    let Some(reply) = &observed.reply else { return Ok(()) };
    let journal = crate::cli::commit_cli::lifetime::journal(reply)
        .ok_or_else(|| Code::VerifyFailed.error("terminal owner reply has no journal"))?;
    let ok = matches!(reply, crate::cli::commit_cli::Reply::Done { .. });
    if !matches!(journal.last(), Some(runs::Event::Finished { ok: outcome, .. }) if *outcome == ok)
        || ok && !state.finished
    {
        return Err(Code::VerifyFailed.error("owner reply differs from its durable outcome"));
    }
    view.run = runs::observed(store, &view.run.summary.id, journal)?;
    view.result = Some(serde_json::to_value(reply).map_err(|e| e.to_string())?);
    view.operation = Some(operation::Status::Finished { ok });
    Ok(())
}

#[cfg(not(test))]
pub(in crate::cli) fn checked_owner(
    store: &Store,
    run: &str,
    host: &str,
    bundle: &str,
    observed: &crate::cli::commit_cli::lifetime::Observation,
) -> Result<bool, Error> {
    let mut current = local(store, run)?;
    current.operation = Some(operation::Status::AwaitingOwner { host: host.into(), bundle: bundle.into() });
    apply_observation(store, &mut current, observed)?;
    match current.operation {
        Some(operation::Status::Finished { ok }) => Ok(ok),
        _ => Err(Code::Blocked.error("owner outcome is not final").with_run(run)),
    }
}

/// Reconciliation is explicit. A read or missing remote record never changes local history.
#[cfg(not(test))]
pub fn reconcile(store: &Store, run: &str) -> Result<View, Error> {
    let control = operation::read(store, run)?.ok_or_else(|| Code::Usage.error("run has no detached operation"))?;
    let State::Owner { host, bundle } = control.state else {
        let current = view(store, run)?;
        if matches!(control.state, State::Stopped | State::Finished { .. } | State::Resolved { .. })
            && !store.is_locked(&format!("operation-active-{}", control.request.env))?
        {
            remove_worker(store, run);
        }
        return Ok(current);
    };
    let observed = crate::cli::commit_cli::lifetime::query(&host, run, &bundle)?;
    let mut current = local(store, run)?;
    current.operation = Some(operation::Status::AwaitingOwner { host: host.clone(), bundle: bundle.clone() });
    apply_observation(store, &mut current, &observed)?;
    let Some(operation::Status::Finished { ok }) = current.operation else {
        return Err(Code::Blocked.error("owner outcome stays unresolved; no local history changed").with_run(run));
    };
    if store.is_locked(&format!("operation-active-{}", control.request.env))? {
        return Err(Code::Blocked.error("local worker still drains; reconcile after it exits").with_run(run));
    }
    let journal =
        crate::cli::commit_cli::lifetime::journal(observed.reply.as_ref().expect("finished requires a reply"))
            .expect("checked owner journal");
    Run::mirror(store, run, journal)?;
    super::resolved(store, run, &host, &bundle, ok, observed.reply.as_ref().expect("checked owner reply"))?;
    remove_worker(store, run);
    Ok(current)
}

#[cfg(test)]
pub fn reconcile(_: &Store, _: &str) -> Result<View, Error> {
    Err(Code::Blocked.error("test runs do not query a production owner"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::commit_cli::{lifetime::Observation, Committed, Reply};
    use crate::engine::runs::{Event, Outcome};
    use crate::store::tests::Scratch;

    #[test]
    fn recent_logs_are_bounded_partial_text_and_never_hide_the_run_on_read_error() {
        let scratch = Scratch::new("operation-log-tail");
        let store = Store::at(&scratch.0);
        let run = Run::create(&store, "prepare local").unwrap();
        let id = run.id().to_string();
        assert!(view(&store, &id).unwrap().logs.is_empty());
        let directory = operation::directory(&store, &id).unwrap();
        assert!(!directory.exists(), "an absent log is not created by observation");
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("stderr.log");
        let mut bytes = vec![b'x'; 32 * 1024];
        bytes.push(b'\n');
        for line in 0..31 {
            bytes.extend_from_slice(format!("line {line}\n").as_bytes());
        }
        bytes.extend_from_slice(b"partial\xff");
        std::fs::write(&path, bytes).unwrap();
        let journal = std::fs::read(store.run(&id)).unwrap();
        let observed = view(&store, &id).unwrap();
        assert_eq!(observed.logs.len(), 12);
        assert_eq!(observed.logs[0], "line 20");
        assert_eq!(observed.logs.last().unwrap(), "partial�");
        assert_eq!(std::fs::read(store.run(&id)).unwrap(), journal);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let failed = view(&store, &id).unwrap();
        assert_eq!(failed.run.summary.id, id);
        assert_eq!(failed.run.summary.outcome, Outcome::Running);
        assert!(failed.logs.is_empty());
        assert!(failed.observation_error.unwrap().contains("not a regular file"));
        run.finish(None).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn a_pre_spawn_log_error_stops_admission_and_finishes_the_same_run() {
        let scratch = Scratch::new("operation-log-error");
        let store = Store::at(&scratch.0);
        let run = Run::create(&store, "prepare local").unwrap();
        let id = run.id().to_string();
        drop(run);
        let request = operation::Request {
            kind: operation::Kind::Prepare,
            env: "local".into(),
            only: Vec::new(),
            moves: Vec::new(),
            plan: None,
            dev: None,
        };
        let control = operation::Control {
            run: id.clone(),
            request_sha256: request.digest().unwrap(),
            request,
            root: "/checkout".into(),
            worker: crate::engine::LayerFile { path: "worker".into(), size: 1, sha256: "a".repeat(64) },
            code: "b".repeat(64),
            state: State::Reserved,
        };
        operation::reserve(&store, &control).unwrap();
        let directory = operation::directory(&store, &id).unwrap();
        std::fs::write(directory.join("worker"), "retained").unwrap();
        std::fs::create_dir(directory.join("stdout.json")).unwrap();
        let primary = operation::launch::mac(&mut std::process::Command::new("unused"), &directory).unwrap_err();
        let error = super::super::unlaunched(&store, &id, Code::Failed.error(primary.clone()));
        assert_eq!(error.message, primary);
        assert_eq!(error.run.as_deref(), Some(id.as_str()));
        assert_eq!(operation::read(&store, &id).unwrap().unwrap().state, State::Stopped);
        assert!(operation::claim(&store, &id, &control.request_sha256).is_err());
        assert!(!directory.join("worker").exists());
        assert!(
            matches!(runs::events(&store, &id).unwrap().last(), Some(Event::Finished { ok: false, error: Some(message), .. }) if message == &primary)
        );
    }

    #[test]
    fn local_completion_seals_output_and_failed_sealing_keeps_the_original_error() {
        let scratch = Scratch::new("operation-local-result");
        let store = Store::at(&scratch.0);
        let request = operation::Request {
            kind: operation::Kind::Prepare,
            env: "live".into(),
            only: Vec::new(),
            moves: Vec::new(),
            plan: None,
            dev: None,
        };
        for (id, fails) in [("2026-10-06-120000", false), ("2026-10-06-120001", true)] {
            let control = operation::Control {
                run: id.into(),
                request_sha256: request.digest().unwrap(),
                request: request.clone(),
                root: "/checkout".into(),
                worker: crate::engine::LayerFile { path: "worker".into(), size: 1, sha256: "a".repeat(64) },
                code: "b".repeat(64),
                state: State::Reserved,
            };
            operation::reserve(&store, &control).unwrap();
            let using = operation::claim(&store, id, &control.request_sha256).unwrap();
            let directory = operation::directory(&store, id).unwrap();
            std::fs::write(directory.join("stdout.json"), r#"{"plan":{"env":"live"}}"#).unwrap();
            if fails {
                std::fs::create_dir(directory.join("result.json")).unwrap();
                let error =
                    super::super::finish_result(&store, id, Err(Code::FetchFailed.error("upstream unavailable")))
                        .unwrap_err();
                assert_eq!(error.code, Code::FetchFailed);
                assert_eq!(error.run.as_deref(), Some(id));
                assert!(error.message.starts_with("upstream unavailable; operation result could not finish:"));
                assert_eq!(operation::read(&store, id).unwrap().unwrap().state, State::Running);
                std::fs::remove_dir(directory.join("result.json")).unwrap();
                super::super::finish_result(&store, id, Err(Code::FetchFailed.error("upstream unavailable")))
                    .unwrap_err();
                assert_eq!(output(&store, id).unwrap().unwrap()["error"]["code"], "fetch_failed");
            } else {
                super::super::finish_result(&store, id, Ok(())).unwrap();
                std::fs::write(directory.join("stdout.json"), r#"{"plan":{"env":"wrong"}}"#).unwrap();
                assert_eq!(output(&store, id).unwrap().unwrap()["plan"]["env"], "live");
                std::fs::write(directory.join("result.json"), r#"{"plan":{"env":"wrong"}}"#).unwrap();
                assert_eq!(output(&store, id).unwrap_err().code, Code::VerifyFailed);
            }
            drop(using);
        }
    }

    #[test]
    fn observing_a_bound_final_owner_keeps_original_local_history_and_unknown_intent_stays_unknown() {
        let scratch = Scratch::new("operation-result-view");
        let store = Store::at(&scratch.0);
        let mut run = Run::create(&store, "apply live").unwrap();
        run.record(&Event::FetchFinished {
            source: "source".into(),
            version: "1".into(),
            params: Vec::new(),
            resolved: "1".into(),
            bytes: 5,
            wall_ms: 1,
        })
        .unwrap();
        let id = run.id().to_string();
        let journal = runs::events(&store, &id).unwrap();
        drop(run);
        let mut view = View {
            run: runs::details(&store, &id).unwrap(),
            operation: Some(operation::Status::AwaitingOwner { host: "publisher".into(), bundle: "a".repeat(64) }),
            observation_error: None,
            result: None,
            logs: Vec::new(),
        };
        let before = std::fs::read(store.run(&id)).unwrap();
        let mut final_journal = journal.clone();
        final_journal.push(Event::Finished { ok: true, error: None, wall_ms: 20 });
        let mut observed = Observation {
            run: id.clone(),
            bundle: "a".repeat(64),
            state: Some(crate::commit::State {
                run: id.clone(),
                bundle: "a".repeat(64),
                pending: None,
                finished: true,
            }),
            active: false,
            reply: Some(Reply::Done { result: Committed::default(), journal: final_journal }),
        };
        apply_observation(&store, &mut view, &observed).unwrap();
        assert_eq!(view.run.summary.outcome, Outcome::Ok);
        assert_eq!(view.run.summary.bytes_fetched, 5, "the remote view replays the original prefix only once");
        assert_eq!(view.operation, Some(operation::Status::Finished { ok: true }));
        assert_eq!(std::fs::read(store.run(&id)).unwrap(), before, "read does not reconcile");
        view.operation = Some(operation::Status::AwaitingOwner { host: "publisher".into(), bundle: "a".repeat(64) });
        observed.state.as_mut().unwrap().pending = Some(crate::commit::Intent {
            mutation: runs::Publication::Uploaded { key: "planner/objects/one".into() },
            expected: None,
            desired: Some("b".repeat(64)),
        });
        observed.reply = None;
        observed.active = true;
        apply_observation(&store, &mut view, &observed).unwrap();
        assert!(
            matches!(view.operation, Some(operation::Status::AwaitingOwner { .. })),
            "a held intent is not a finished failure"
        );
        observed.active = false;
        apply_observation(&store, &mut view, &observed).unwrap();
        assert!(matches!(view.operation, Some(operation::Status::UnknownOwner { .. })));
        assert_eq!(std::fs::read(store.run(&id)).unwrap(), before);
        view.operation = Some(operation::Status::AwaitingOwner { host: "publisher".into(), bundle: "a".repeat(64) });
        observed.state = None;
        observed.reply = None;
        apply_observation(&store, &mut view, &observed).unwrap();
        assert!(
            matches!(view.operation, Some(operation::Status::AwaitingOwner { .. })),
            "absence cannot disprove delayed dispatch"
        );
        observed.bundle = "b".repeat(64);
        assert!(apply_observation(&store, &mut view, &observed).is_err());
    }

    #[test]
    fn reconciliation_keeps_the_bundle_and_seals_the_owner_result_over_a_transport_error() {
        let scratch = Scratch::new("operation-sealed-result");
        let store = Store::at(&scratch.0);
        let run = Run::create(&store, "apply live").unwrap();
        let id = run.id().to_string();
        let mut journal = runs::events(&store, &id).unwrap();
        drop(run);
        let request = operation::Request {
            kind: operation::Kind::Apply,
            env: "live".into(),
            only: Vec::new(),
            moves: Vec::new(),
            plan: Some(crate::engine::LayerFile { path: "plan.json".into(), size: 1, sha256: "d".repeat(64) }),
            dev: None,
        };
        let control = operation::Control {
            run: id.clone(),
            request_sha256: request.digest().unwrap(),
            request,
            root: "/checkout".into(),
            worker: crate::engine::LayerFile { path: "worker".into(), size: 1, sha256: "b".repeat(64) },
            code: "c".repeat(64),
            state: State::Reserved,
        };
        operation::reserve(&store, &control).unwrap();
        let using = operation::claim(&store, &id, &control.request_sha256).unwrap();
        operation::handoff(&store, &id, "publisher", &"a".repeat(64)).unwrap();
        drop(using);
        std::fs::write(
            operation::directory(&store, &id).unwrap().join("stdout.json"),
            r#"{"error":{"message":"transport lost"}}"#,
        )
        .unwrap();
        journal.push(Event::Finished { ok: true, error: None, wall_ms: 20 });
        Run::mirror(&store, &id, &journal).unwrap();
        let reply = Reply::Done { result: Committed::default(), journal };
        super::super::resolved(&store, &id, "publisher", &"a".repeat(64), true, &reply).unwrap();
        let resolved = operation::read(&store, &id).unwrap().unwrap();
        assert!(
            matches!(resolved.state, State::Resolved { host, bundle, ok: true, .. } if host == "publisher" && bundle == "a".repeat(64))
        );
        assert_eq!(view(&store, &id).unwrap().result.unwrap()["status"], "done");
        assert!(!view(&store, &id).unwrap().result.unwrap().as_object().unwrap().contains_key("journal"));
        super::super::resolved(&store, &id, "publisher", &"a".repeat(64), true, &reply).unwrap();
        let sealed = std::fs::read(operation::directory(&store, &id).unwrap().join("owner-result.json")).unwrap();
        assert!(super::super::resolved(&store, &id, "other-owner", &"a".repeat(64), true, &reply).is_err());
        assert_eq!(
            std::fs::read(operation::directory(&store, &id).unwrap().join("owner-result.json")).unwrap(),
            sealed
        );
        std::fs::write(operation::directory(&store, &id).unwrap().join("owner-result.json"), "changed").unwrap();
        assert_eq!(view(&store, &id).unwrap_err().code, Code::VerifyFailed);
    }
}
