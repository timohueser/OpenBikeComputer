//! Admission uses one fixed worker and owner setup. Reads cannot resolve an uncertain dispatch.

use crate::operation::launch::bounded_output;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{sha, Bundle, Reply};
use crate::commit::{durable, Owner, State};
use crate::engine::runs::{check_id, Event};
use crate::store::{sha256_hex, Store};

const WORKER: &str = "/opt/obc-data/bin/obc-data-plumbing";
const ENVIRONMENT: &str = "/etc/obc-data/owner.env";

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(in crate::cli) struct Observation {
    pub run: String,
    pub bundle: String,
    pub state: Option<State>,
    pub active: bool,
    pub reply: Option<Reply>,
}

pub(in crate::cli) fn unit(run: &str, digest: &str) -> Result<String, String> {
    check_id(run)?;
    if !sha(digest) {
        return Err("commit bundle digest is not SHA-256".into());
    }
    Ok(format!("obc-data-commit-{run}-{digest}"))
}

pub(in crate::cli) fn incoming(run: &str, digest: &str) -> Result<std::path::PathBuf, String> {
    unit(run, digest)?;
    Ok(Path::new("/var/lib/obc-data/incoming").join(run).join(digest))
}

pub(in crate::cli) fn observe(store: &Store, run: &str, digest: &str) -> Result<Observation, String> {
    unit(run, digest)?;
    let read = |suffix: &str| -> Result<Option<Vec<u8>>, String> {
        match std::fs::read(store.root().join("commits").join(format!("{run}.{suffix}"))) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.to_string()),
        }
    };
    let state: Option<State> =
        read("json")?.map(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string())).transpose()?;
    if state.as_ref().is_some_and(|state| state.run != run || state.bundle != digest) {
        return Err("owner state is bound to another operation bundle".into());
    }
    let mut reply =
        read("reply")?.map(|bytes| serde_json::from_slice(&bytes).map_err(|e| e.to_string())).transpose()?;
    if reply.is_some() && state.is_none() {
        return Err("owner reply has no admitted bundle binding".into());
    }
    let active = Owner::active(&store.root().join("commits"), run, digest)?;
    if active && state.is_none() {
        return Err("active owner has no admitted state".into());
    }
    // A crash after sealing publication can precede the separate approval write and reply.
    if reply.is_none() && !active && state.as_ref().is_some_and(|state| state.finished && state.pending.is_none()) {
        if let Some(bytes) = read("result")? {
            let journal = crate::engine::runs::events(store, run)?;
            if !matches!(journal.last(), Some(Event::Finished { ok: true, .. })) {
                return Err("sealed publication has no successful owner journal".into());
            }
            let mut result: super::Committed = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            result.approval = match crate::approval::read(store) {
                Ok(observed)
                    if observed.current.as_ref().is_some_and(|current| {
                        current.run == run && current.bundle == digest && current.publication == sha256_hex(&bytes)
                    }) =>
                {
                    crate::approval::Outcome::Recorded {
                        sha256: observed.sha256.unwrap(),
                        unavailable: observed.current.unwrap().unavailable,
                    }
                }
                _ => crate::approval::Outcome::Unresolved {
                    reason: "publication is sealed; its separate approval outcome has no durable reply".into(),
                },
            };
            reply = Some(Reply::Done { result, journal });
        }
    }
    Ok(Observation { run: run.into(), bundle: digest.into(), state, active, reply })
}

/// The incoming path, request and service name all bind the same immutable run and bundle.
pub(in crate::cli) fn admit(store: &Store, directory: &Path, digest: &str) -> Result<(), String> {
    if !cfg!(target_os = "linux") {
        return Err("commit admission needs the configured Linux VPS".into());
    }
    let bytes = std::fs::read(directory.join("bundle.json")).map_err(|e| e.to_string())?;
    if sha256_hex(&bytes) != digest {
        return Err("commit admission bundle checksum differs".into());
    }
    let bundle: Bundle = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    super::validate(&bundle).map_err(|error| error.message)?;
    crate::approval::check_owner(store, &bundle.approval)?;
    if directory != incoming(&bundle.run, digest)? {
        return Err("commit admission is outside the fixed incoming directory".into());
    }
    let environment = crate::operation::launch::private_file(Path::new(ENVIRONMENT))?;
    // Reserve the immutable identity before the service command can arrive late.
    // An existing unknown intent still excludes admission of every later owner.
    let owner = Owner::open(&store.root().join("commits"), &bundle.run, &bytes)?;
    let observed = observe(store, &bundle.run, digest)?;
    if observed.reply.is_some() {
        return Ok(());
    }
    let name = unit(&bundle.run, digest)?;
    let mut command = crate::operation::launch::service(&name, directory, directory, &environment, false);
    command.arg(WORKER).args(["commit", directory.to_str().ok_or("incoming path is not UTF-8")?, digest]);
    drop(owner);
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    // A second admission may find the same admitted unit. Nothing is restarted.
    let active = Command::new("systemctl").args(["is-active", &name]).output().map_err(|e| e.to_string())?;
    if active.status.success() {
        return Ok(());
    }
    Err("commit service admission failed or is uncertain; inspect its bound owner status".into())
}

pub(in crate::cli) fn persist_reply(store: &Store, run: &str, reply: &Reply) -> Result<(), String> {
    check_id(run)?;
    durable(
        &store.root().join("commits").join(format!("{run}.reply")),
        &serde_json::to_vec(reply).map_err(|e| e.to_string())?,
    )
}

pub(in crate::cli) fn journal(reply: &Reply) -> Option<&[Event]> {
    match reply {
        Reply::Done { journal, .. } => Some(journal),
        Reply::Failed { journal, .. } => journal.as_deref(),
    }
}

#[cfg(not(test))]
pub(in crate::cli) fn query(host: &str, run: &str, digest: &str) -> Result<Observation, String> {
    super::host(host).map_err(|error| error.message)?;
    unit(run, digest)?;
    let mut command = query_command(host);
    command.args(["commit-status", run, digest]);
    let output = bounded_output(&mut command, Duration::from_secs(30))?;
    let observed: Observation = serde_json::from_slice(&output).map_err(|e| e.to_string())?;
    if observed.run != run
        || observed.bundle != digest
        || observed.state.as_ref().is_some_and(|state| state.run != run || state.bundle != digest)
    {
        return Err("owner status names another run or bundle".into());
    }
    Ok(observed)
}

#[cfg(any(not(test), unix))]
fn query_command(host: &str) -> Command {
    if host == "local" {
        Command::new(WORKER)
    } else {
        let mut command = Command::new("ssh");
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectionAttempts=1",
            "-o",
            "ConnectTimeout=10",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=2",
            host,
            WORKER,
        ]);
        command
    }
}

#[cfg(not(test))]
pub(super) fn approval(host: &str) -> Result<crate::approval::Observation, String> {
    super::host(host).map_err(|error| error.message)?;
    let output = bounded_output(query_command(host).arg("commit-approval"), Duration::from_secs(30))?;
    let observed: crate::approval::Observation = serde_json::from_slice(&output).map_err(|error| error.to_string())?;
    observed.check()?;
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::Instant;

    #[cfg(unix)]
    #[test]
    fn owner_observation_is_noninteractive_and_has_a_real_command_deadline() {
        let command = query_command("publisher");
        let args: Vec<_> = command.get_args().map(|arg| arg.to_string_lossy().to_string()).collect();
        for option in [
            "BatchMode=yes",
            "ConnectionAttempts=1",
            "ConnectTimeout=10",
            "ServerAliveInterval=5",
            "ServerAliveCountMax=2",
        ] {
            assert!(args.iter().any(|arg| arg == option));
        }
        let mut normal = Command::new("sh");
        normal.args(["-c", "printf status"]);
        assert_eq!(bounded_output(&mut normal, Duration::from_secs(5)).unwrap(), b"status");
        let mut stuck = Command::new("sh");
        stuck.args(["-c", "sleep 60"]);
        let started = Instant::now();
        assert!(bounded_output(&mut stuck, Duration::from_millis(50)).unwrap_err().contains("deadline expired"));
        assert!(started.elapsed() < Duration::from_secs(5), "a wedged status reader must return unresolved");
        let mut inherited = Command::new("sh");
        inherited.args(["-c", "printf status; sleep 60 &"]);
        let started = Instant::now();
        assert!(bounded_output(&mut inherited, Duration::from_millis(50)).unwrap_err().contains("deadline expired"));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "an exited reader's child cannot hold output collection open"
        );
    }

    #[test]
    fn sealed_publication_without_reply_exposes_separate_unresolved_approval_without_writes() {
        let scratch = crate::store::tests::Scratch::new("publication-without-approval-reply");
        let store = Store::at(&scratch.0);
        let run = crate::engine::runs::Run::create(&store, "apply live").unwrap();
        let id = run.id().to_string();
        let bytes = b"bundle";
        let digest = sha256_hex(bytes);
        let mut owner = Owner::open(&store.root().join("commits"), &id, bytes).unwrap();
        let result = serde_json::to_vec(&super::super::Committed::default()).unwrap();
        durable(&store.root().join("commits").join(format!("{id}.result")), &result).unwrap();
        run.finish(None).unwrap();
        owner.finish().unwrap();
        drop(owner);
        let observed = observe(&store, &id, &digest).unwrap();
        let Some(Reply::Done { result, .. }) = observed.reply else { panic!("sealed publication was hidden") };
        assert!(matches!(result.approval, crate::approval::Outcome::Unresolved { .. }));
        assert!(!store.root().join("commits").join(format!("{id}.reply")).exists());
        assert!(!store.root().join("commits/current.approval").exists());
    }

    #[test]
    fn missing_owner_evidence_is_not_completion_and_identity_cannot_change() {
        let scratch = crate::store::tests::Scratch::new("owner-observation");
        let store = Store::at(&scratch.0);
        let run = "2026-10-06-120000";
        let bytes = b"bundle";
        let digest = sha256_hex(bytes);
        assert!(observe(&store, run, &digest).unwrap().state.is_none());
        let owner = Owner::open(&store.root().join("commits"), run, bytes).unwrap();
        let observed = observe(&store, run, &digest).unwrap();
        assert!(!observed.state.unwrap().finished);
        assert!(observed.reply.is_none());
        assert!(observe(&store, run, &"b".repeat(64)).is_err());
        drop(owner);
        assert!(observe(&store, run, &digest).unwrap().reply.is_none());
    }
}
