//! Read-only operation results and worker logs.

use schemars::JsonSchema;
use serde::Serialize;

use crate::cli::{Code, Error};
use crate::engine::runs;
use crate::operation::{self, State};
use crate::store::Store;

#[derive(Debug, Serialize, JsonSchema)]
pub struct View {
    pub run: runs::Details,
    pub operation: Option<operation::Status>,
    pub observation_error: Option<String>,
    pub result: Option<serde_json::Value>,
    /// Recent worker stderr. Reading it does not change the run.
    pub logs: Vec<String>,
}

/// Observation never changes the control or the journal.
pub fn view(store: &Store, run: &str) -> Result<View, Error> {
    let mut view = local(store, run)?;
    if matches!(view.operation, Some(operation::Status::Finished { .. })) {
        view.result = output(store, run)?;
    }
    if matches!(
        view.operation,
        Some(operation::Status::Starting | operation::Status::Running | operation::Status::Stopping)
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
        if let State::Finished { ref result, .. } = control.state {
            let path = directory.join(&result.path);
            let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
            if bytes.len() as u64 != result.size || crate::store::sha256_hex(&bytes) != result.sha256 {
                return Err(Code::VerifyFailed.error("sealed operation result bytes changed"));
            }
            return serde_json::from_slice(&bytes).map(Some).map_err(|e| Code::Failed.error(e.to_string()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::runs::{Event, Outcome, Run};
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

    #[cfg(unix)]
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
            content: None,
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
        let primary = operation::launch::detach(&mut std::process::Command::new("unused"), &directory).unwrap_err();
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
            content: None,
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
}
