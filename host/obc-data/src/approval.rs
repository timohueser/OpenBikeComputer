//! One current publication approval, with the latest checked tools for each native context.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::engine::{Profile, ResolvedRust};
use crate::store::{sha256_hex, Store};

mod resolve;
pub(crate) use resolve::review;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeBinding {
    pub target: serde_json::Value,
    pub builder: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Role {
    pub rust: Option<ResolvedRust>,
    pub source_config: String,
    pub execution: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub target: String,
    pub profile: Profile,
    pub roles: BTreeMap<String, Role>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Current {
    pub owner: String,
    pub bucket: String,
    pub config: String,
    pub executions: Vec<Execution>,
    pub unavailable: Option<String>,
    pub run: String,
    pub bundle: String,
    /// The sealed publication result has no approval digest.
    pub publication: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub owner: String,
    /// None is a successful observation of absence, never a failed read.
    pub sha256: Option<String>,
    pub current: Option<Current>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Review {
    Ready {
        owner: String,
        expected: Option<String>,
        config: String,
        executions: Vec<Execution>,
        /// Retained inputs can be applied when unused acquisition tools are absent.
        unavailable: Option<String>,
    },
    Unavailable {
        reason: String,
        owner: Option<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum Outcome {
    #[default]
    NotRequested,
    Recorded {
        sha256: String,
        unavailable: Option<String>,
    },
    Unavailable {
        reason: String,
    },
    /// Publication succeeds independently of this owner-local durable write.
    Unresolved {
        reason: String,
    },
}

impl Outcome {
    pub fn summary(&self) -> String {
        match self {
            Self::NotRequested => "Automatic approval was not requested.".into(),
            Self::Recorded { sha256, unavailable } => {
                let mut text = format!("Automatic approval recorded: {}.", &sha256[..sha256.len().min(8)]);
                if let Some(reason) = unavailable {
                    text.push_str(&format!(" Current execution unavailable: {reason}"));
                }
                text
            }
            Self::Unavailable { reason } => format!("Automatic approval unavailable: {reason}"),
            Self::Unresolved { reason } => format!("Publication is complete. Automatic approval unresolved: {reason}"),
        }
    }
}

fn path(store: &Store) -> std::path::PathBuf {
    store.root().join("commits").join("current.approval")
}

pub fn read(store: &Store) -> Result<Observation, String> {
    let owner = owner(store)?;
    let bytes = match std::fs::read(path(store)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Observation { owner, sha256: None, current: None });
        }
        Err(error) => return Err(format!("read current approval: {error}")),
    };
    let current: Current = serde_json::from_slice(&bytes).map_err(|error| format!("current approval: {error}"))?;
    current.check()?;
    if current.owner != owner {
        return Err("current approval belongs to another configured owner".into());
    }
    checked_publication(store, &current)?;
    Ok(Observation { owner, sha256: Some(sha256_hex(&bytes)), current: Some(current) })
}

fn checked_publication(store: &Store, current: &Current) -> Result<(), String> {
    let directory = store.root().join("commits");
    let state: crate::commit::State = serde_json::from_slice(
        &std::fs::read(directory.join(format!("{}.json", current.run))).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    let result = std::fs::read(directory.join(format!("{}.result", current.run))).map_err(|error| error.to_string())?;
    if state.run != current.run
        || state.bundle != current.bundle
        || !state.finished
        || state.pending.is_some()
        || sha256_hex(&result) != current.publication
        || !matches!(
            crate::engine::runs::events(store, &current.run)?.last(),
            Some(crate::engine::runs::Event::Finished { ok: true, .. })
        )
    {
        return Err("current approval has no complete sealed owner publication".into());
    }
    Ok(())
}

impl Review {
    pub(crate) fn check(&self) -> Result<(), String> {
        let pinned = match self {
            Self::Ready { owner, .. } => Some(owner),
            Self::Unavailable { owner, .. } => owner.as_ref(),
        }
        .ok_or_else(|| format!("the plan has no checked publication owner identity: {}", self.summary()))?;
        if !digest(pinned) {
            return Err("the reviewed publication owner identity is invalid".into());
        }
        if let Self::Ready { expected, config, executions, .. } = self {
            if !digest(config) || expected.as_ref().is_some_and(|value| !digest(value)) {
                return Err("invalid reviewed approval identity".into());
            }
            check_executions(executions)?;
        }
        Ok(())
    }

    pub fn summary(&self) -> String {
        match self {
            Self::Ready { owner, expected, config, executions, unavailable } => {
                let previous = expected.as_deref().unwrap_or("absent");
                let short = |value: &str| value.get(..8).unwrap_or(value).to_string();
                let mut text = format!(
                    "AUTO APPROVAL · owner {} · prior {} · config {} · {} checked contexts",
                    short(owner),
                    short(previous),
                    short(config),
                    executions.len()
                );
                if let Some(reason) = unavailable {
                    text.push_str(&format!("; current execution unavailable: {reason}"));
                }
                text
            }
            Self::Unavailable { reason, .. } => format!("AUTO APPROVAL unavailable: {reason}"),
        }
    }
}

fn owner_digest(machine: &[u8], root: &std::path::Path) -> Result<String, String> {
    let root = root.canonicalize().map_err(|error| format!("configure the fixed owner store: {error}"))?;
    let bytes =
        serde_json::to_vec(&("obc-data publication owner", machine, root)).map_err(|error| error.to_string())?;
    Ok(sha256_hex(&bytes))
}

fn owner(store: &Store) -> Result<String, String> {
    #[cfg(test)]
    {
        owner_digest(b"explicit fixture owner", store.root())
    }
    #[cfg(not(test))]
    {
        if !cfg!(target_os = "linux") || store.root() != std::path::Path::new("/var/lib/obc-data/store") {
            return Err("approval requires the configured Linux publication owner and fixed store".into());
        }
        let machine = std::fs::read_to_string("/etc/machine-id")
            .map_err(|error| format!("read configured Linux owner identity: {error}"))?;
        let machine = machine.trim();
        if machine.len() != 32 || !machine.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err("configured Linux publication owner has no valid machine identity".into());
        }
        owner_digest(machine.as_bytes(), store.root())
    }
}

fn digest(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

impl Observation {
    pub(crate) fn check(&self) -> Result<(), String> {
        if !digest(&self.owner) {
            return Err("current approval observation has no fixed owner identity".into());
        }
        match (&self.sha256, &self.current) {
            (None, None) => Ok(()),
            (Some(sha256), Some(current)) if digest(sha256) && current.owner == self.owner => current.check(),
            _ => Err("current approval observation has inconsistent presence or checksum".into()),
        }
    }
}

impl Execution {
    fn context(&self) -> (&str, Profile) {
        (&self.target, self.profile)
    }

    fn check(&self) -> Result<(), String> {
        if self.target.is_empty()
            || self.roles.is_empty()
            || self.target.chars().any(char::is_control)
            || self.roles.values().any(|role| !digest(&role.source_config) || !digest(&role.execution))
        {
            return Err("approval has an invalid execution context or role identity".into());
        }
        Ok(())
    }
}

impl Current {
    fn check(&self) -> Result<(), String> {
        crate::engine::runs::check_id(&self.run)?;
        if !digest(&self.owner) || !digest(&self.config) || !digest(&self.bundle) || !digest(&self.publication) {
            return Err("approval has an invalid publication binding".into());
        }
        check_executions(&self.executions)
    }
}

fn check_executions(executions: &[Execution]) -> Result<(), String> {
    for (at, execution) in executions.iter().enumerate() {
        execution.check()?;
        if executions[..at].iter().any(|previous| previous.context() == execution.context()) {
            return Err("approval repeats a native target/profile context".into());
        }
    }
    Ok(())
}

pub(crate) fn check_owner(store: &Store, review: &Review) -> Result<(), String> {
    review.check()?;
    let pinned = match review {
        Review::Ready { owner, .. } => Some(owner),
        Review::Unavailable { owner, .. } => owner.as_ref(),
    }
    .ok_or("the plan has no checked publication owner identity; observe the configured owner and plan again")?;
    if &owner(store)? != pinned {
        return Err("configured publication owner changed since review".into());
    }
    Ok(())
}

/// Called under the final owner lock, before any publication mutation.
pub(crate) fn recheck(store: &Store, review: &Review) -> Result<(), String> {
    check_owner(store, review)?;
    if let Review::Ready { expected, .. } = review {
        if read(store)?.sha256 != *expected {
            return Err("current approval changed before the owner lock; review a new plan".into());
        }
    }
    Ok(())
}

/// Only a sealed complete publication reaches this write. A retry accepts the exact same binding.
pub(crate) fn record(
    store: &Store,
    review: &Review,
    bucket: &str,
    run: &str,
    bundle: &str,
    publication: &str,
) -> Outcome {
    let Review::Ready { expected, config, executions, unavailable, .. } = review else {
        let Review::Unavailable { reason, .. } = review else { unreachable!() };
        return Outcome::Unavailable { reason: reason.clone() };
    };
    let current = Current {
        owner: match review {
            Review::Ready { owner, .. } => owner.clone(),
            _ => unreachable!(),
        },
        bucket: bucket.into(),
        config: config.clone(),
        executions: executions.clone(),
        unavailable: unavailable.clone(),
        run: run.into(),
        bundle: bundle.into(),
        publication: publication.into(),
    };
    let write = (|| {
        current.check()?;
        checked_publication(store, &current)?;
        let observed = read(store)?;
        if observed.current.as_ref() == Some(&current) {
            return Ok(observed.sha256.unwrap());
        }
        if observed.sha256 != *expected {
            return Err("approval changed after publication; checked publication remains complete".into());
        }
        let bytes = serde_json::to_vec(&current).map_err(|error| error.to_string())?;
        crate::commit::durable(&path(store), &bytes)?;
        Ok(sha256_hex(&bytes))
    })();
    match write {
        Ok(sha256) => Outcome::Recorded { sha256, unavailable: unavailable.clone() },
        Err(reason) => Outcome::Unresolved { reason },
    }
}

/// Keep an already checked other context only when the current declarations reproduce its witness.
pub(crate) fn retain(
    previous: &Observation,
    bucket: &str,
    config: &str,
    checked: Option<Execution>,
    mut comparable: impl FnMut(&Execution) -> bool,
) -> Vec<Execution> {
    let mut executions = previous
        .current
        .as_ref()
        .filter(|current| current.bucket == bucket && current.config == config)
        .map(|current| {
            current
                .executions
                .iter()
                .filter(|entry| {
                    checked.as_ref().is_none_or(|checked| checked.context() != entry.context()) && comparable(entry)
                })
                .cloned()
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(checked) = checked {
        executions.push(checked);
    }
    let order = |profile| match profile {
        Profile::Dev => 0,
        Profile::Release => 1,
    };
    executions.sort_by(|a, b| (&a.target, order(a.profile)).cmp(&(&b.target, order(b.profile))));
    executions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Rust;
    use crate::store::tests::Scratch;

    fn sha(value: &str) -> String {
        sha256_hex(value.as_bytes())
    }

    fn execution(target: &str, source: &str, compiler: &str) -> Execution {
        Execution {
            target: target.into(),
            profile: Profile::Dev,
            roles: BTreeMap::from([(
                "planning/maps".into(),
                Role {
                    rust: Some(ResolvedRust { target: target.into(), build: Rust::Native { profile: Profile::Dev } }),
                    source_config: sha(source),
                    execution: sha(compiler),
                },
            )]),
        }
    }

    #[test]
    fn fixed_owner_identity_changes_before_publication_when_the_machine_or_store_changes() {
        let scratch = Scratch::new("approval-owner-binding");
        let root = &scratch.0;
        let first = owner_digest(b"fixture machine A", root).unwrap();
        assert_eq!(owner_digest(b"fixture machine A", &root.join(".")).unwrap(), first);
        assert_ne!(owner_digest(b"fixture machine B", root).unwrap(), first);
        std::fs::create_dir(root.join("other")).unwrap();
        assert_ne!(owner_digest(b"fixture machine A", &root.join("other")).unwrap(), first);
        let store = Store::at(root);
        let observed = read(&store).unwrap();
        let review = Review::Unavailable { reason: "unused tools unavailable".into(), owner: Some(observed.owner) };
        check_owner(&store, &review).unwrap();
        let other = Review::Unavailable { reason: "unused tools unavailable".into(), owner: Some(first) };
        assert!(check_owner(&store, &other).unwrap_err().contains("owner changed"));
        assert!(!root.join("commits").exists(), "read/target validation does not admit a mutation");
    }

    #[test]
    fn same_scope_keeps_checked_other_targets_but_replaces_the_current_compiler() {
        let linux = execution("linux", "linux-source", "checked-vps");
        let mac = execution("mac", "mac-source", "old-mac-compiler");
        let previous = Observation {
            owner: sha("owner"),
            sha256: Some(sha("record")),
            current: Some(Current {
                owner: sha("owner"),
                bucket: "bucket".into(),
                config: sha("settings"),
                executions: vec![linux.clone(), mac],
                unavailable: None,
                run: "2026-10-07-000000".into(),
                bundle: sha("bundle"),
                publication: sha("complete publication"),
            }),
        };
        let checked = execution("mac", "mac-source", "new-mac-compiler");
        let mut witnessed = Vec::new();
        let entries = retain(&previous, "bucket", &sha("settings"), Some(checked.clone()), |entry| {
            witnessed.push(entry.target.clone());
            entry.roles["planning/maps"].source_config == sha("linux-source")
        });
        assert_eq!(entries, [linux.clone(), checked]);
        assert_eq!(witnessed, ["linux"]);
        assert!(retain(&previous, "bucket", &sha("new settings"), None, |_| true).is_empty());
        assert!(retain(&previous, "other bucket", &sha("settings"), None, |_| true).is_empty());
        assert!(
            retain(&previous, "bucket", &sha("settings"), None, |_| false).is_empty(),
            "stale source cannot roll back the current record"
        );
        assert_eq!(
            retain(&previous, "bucket", &sha("settings"), None, |entry| entry == &linux),
            [linux],
            "unused unavailable local tools do not fabricate a new execution approval"
        );
    }

    #[test]
    fn exact_cas_and_separate_sealed_publication_survive_retry_and_failed_approval_write() {
        let scratch = Scratch::new("approval-cas");
        let store = Store::at(&scratch.0);
        let run = crate::engine::runs::Run::create(&store, "apply live").unwrap();
        let id = run.id().to_string();
        let bundle = sha("reviewed bundle");
        let mut guard = crate::commit::Owner::open(&store.root().join("commits"), &id, b"reviewed bundle").unwrap();
        let initial = read(&store).unwrap();
        assert_eq!(initial, Observation { owner: owner(&store).unwrap(), sha256: None, current: None });
        let review = Review::Ready {
            owner: initial.owner.clone(),
            expected: None,
            config: sha("settings"),
            executions: vec![execution("linux", "source", "compiler")],
            unavailable: None,
        };
        recheck(&store, &review).unwrap();
        let publication = br#"{"uploaded":[],"switched":[],"removed":[]}"#;
        let publication_sha = sha256_hex(publication);
        let result_path = store.root().join("commits").join(format!("{id}.result"));
        crate::commit::durable(&result_path, publication).unwrap();
        run.finish(None).unwrap();
        guard.finish().unwrap();
        let outcome = record(&store, &review, "bucket", &id, &bundle, &publication_sha);
        let Outcome::Recorded { ref sha256, .. } = outcome else { panic!("{outcome:?}") };
        let observed = read(&store).unwrap();
        assert_eq!(observed.sha256.as_ref(), Some(sha256));
        assert_eq!(observed.current.as_ref().unwrap().publication, publication_sha);
        assert_eq!(std::fs::read(&result_path).unwrap(), publication, "publication has no approval digest cycle");
        assert!(recheck(&store, &review).unwrap_err().contains("changed before"));
        assert_eq!(record(&store, &review, "bucket", &id, &bundle, &publication_sha), outcome);
        assert!(matches!(
            record(&store, &review, "bucket", &id, &sha("changed bundle"), &publication_sha),
            Outcome::Unresolved { .. }
        ));
        assert_eq!(read(&store).unwrap(), observed);
        std::fs::remove_file(path(&store)).unwrap();
        std::fs::create_dir(path(&store)).unwrap();
        assert!(matches!(
            record(&store, &review, "bucket", &id, &bundle, &publication_sha),
            Outcome::Unresolved { .. }
        ));
        assert_eq!(std::fs::read(&result_path).unwrap(), publication, "checked publication remains a separate success");
        drop(guard);
        std::fs::remove_dir(path(&store)).unwrap();
        crate::commit::durable(&path(&store), &serde_json::to_vec(&observed.current.unwrap()).unwrap()).unwrap();
        let next = crate::engine::runs::Run::create(&store, "apply live").unwrap();
        let next_guard = crate::commit::Owner::open(&store.root().join("commits"), next.id(), b"next bundle").unwrap();
        drop(next_guard);
        next.finish(Some("fixture ends before publication")).unwrap();
    }
}
