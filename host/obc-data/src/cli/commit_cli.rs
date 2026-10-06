//! The product-free final writer, with the original operation journal.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
#[cfg(not(test))]
use std::process::Command;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use super::apply_cli::{self, Applied, Wait, WAIT};
use super::build_cli::BuiltRelease;
use super::{Code, Error};
use crate::commit::{durable, Intent, Owner};
use crate::engine::runs::{Event, Phase, Publication, Run};
use crate::live::{Live, Remote};
use crate::r2::{Scratch, Upload};
use crate::sources::Source;
use crate::store::{hash_file, sha256_hex, Store};

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Bundle {
    run: String,
    bucket: String,
    journal: Vec<Event>,
    expected: BTreeMap<String, Option<String>>,
    next: Live,
    sources: Vec<Source>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Committed {
    uploaded: Vec<String>,
    switched: Vec<BuiltRelease>,
    removed: Vec<crate::r2::Object>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
enum Reply {
    Done { result: Committed, journal: Vec<Event> },
    Failed { error: Error, journal: Option<Vec<Event>> },
}

impl Committed {
    pub fn apply(self, applied: &mut Applied) {
        applied.uploaded = self.uploaded;
        applied.switched = self.switched;
        applied.removed = self.removed;
    }
}

/// Consent applies to these exact pointer bytes, including publication fields.
pub(super) fn expected(
    plan: &super::build_cli::EnvPlan,
    products: &[&dyn crate::product::Product],
) -> Result<BTreeMap<String, Option<String>>, Error> {
    products
        .iter()
        .filter(|product| product.pointer().is_some())
        .map(|product| {
            let observed = plan
                .live
                .iter()
                .find(|observed| observed.product == product.name())
                .ok_or_else(|| Code::PlanOutdated.error("saved plan has no original pointer observation"))?;
            Ok((format!("{}/catalog.json", product.prefix()), observed.observed.clone()))
        })
        .collect()
}

pub(super) fn pack(
    directory: &Path,
    store: &Store,
    run: &Run,
    expected: BTreeMap<String, Option<String>>,
    mut next: Live,
    sources: Vec<Source>,
    remote: &Remote,
) -> Result<String, Error> {
    next.products.retain(|product| expected.contains_key(&format!("{}/catalog.json", product.prefix)));
    let bundle = Bundle {
        run: run.id().into(),
        bucket: remote.describe().into(),
        journal: crate::engine::runs::events(store, run.id())?,
        expected,
        next,
        sources,
    };
    let scratch = Scratch::new()?;
    let files = apply_cli::files(store, &scratch, &bundle.next)?;
    let listed: BTreeMap<_, _> =
        bundle.next.list(remote)?.into_iter().map(|object| (object.key, object.bytes)).collect();
    let objects: BTreeMap<_, _> = files
        .iter()
        .filter(|file| file.size != listed.get(&file.key).copied())
        .filter_map(|file| {
            let digest = file.sha256.as_ref()?;
            (file.path == store.object(digest))
                .then(|| (digest.clone(), file.size.expect("an object has its receipt size")))
        })
        .collect();
    let output = Store::at(directory);
    for (digest, size) in objects {
        let source = store.object(&digest);
        if !source.exists() {
            continue;
        }
        if hash_file(&source)? != (digest.clone(), size) {
            return Err(Code::VerifyFailed.error("commit input differs from its receipt"));
        }
        let target = output.object(&digest);
        std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::hard_link(source, target).map_err(|e| e.to_string())?;
    }
    let bytes = serde_json::to_vec(&bundle).map_err(|e| e.to_string())?;
    durable(&directory.join("bundle.json"), &bytes)?;
    Ok(sha256_hex(&bytes))
}

fn sha(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate(bundle: &Bundle) -> Result<(), Error> {
    crate::engine::runs::check_id(&bundle.run)?;
    let segment = |value: &str| {
        !value.is_empty()
            && !matches!(value, "." | "..")
            && !value.contains(['/', '\\'])
            && !value.chars().any(char::is_control)
    };
    for (key, record) in &bundle.next.inputs {
        if !crate::is_kebab(&key.source) || !segment(&key.version) || !sha(&key.digest) {
            return Err(Code::Usage.error("invalid input-copy key"));
        }
        if let Some(record) = record {
            record.validate(key)?;
        }
    }
    for read in crate::input_copy::reads(&bundle.next)? {
        if bundle.next.inputs.contains_key(&read.key)
            || bundle.sources.iter().any(|source| source.id == read.key.source && source.r2_copy)
        {
            let record = bundle.next.inputs.get(&read.key).and_then(Option::as_ref).ok_or_else(|| {
                Code::Blocked.error(format!("commit lacks required input-copy metadata {}", read.key.path()))
            })?;
            if record.files.iter().map(|file| file.name.as_str()).collect::<Vec<_>>()
                != read.files.iter().map(String::as_str).collect::<Vec<_>>()
            {
                return Err(Code::VerifyFailed.error("commit copy selection differs from its release read"));
            }
        }
    }
    if bundle.expected.values().flatten().any(|digest| !sha(digest)) {
        return Err(Code::Usage.error("invalid observed pointer identity"));
    }
    let mut names = BTreeSet::new();
    for product in &bundle.next.products {
        let known = matches!(
            (product.product.as_str(), product.prefix.as_str()),
            ("maps", "cell-catalog") | ("planner", "planner")
        );
        #[cfg(test)]
        let known = known || product.product == "test" && product.prefix == "test";
        if !known || !names.insert(product.prefix.as_str()) {
            return Err(Code::Usage.error("commit bundle has an unknown or duplicate publication"));
        }
        if !bundle.expected.contains_key(&format!("{}/catalog.json", product.prefix)) {
            return Err(Code::Usage.error("commit bundle lacks its expected pointer"));
        }
        if let Some((id, release)) = &product.release {
            release.check_named()?;
            if release.layers.iter().flat_map(|layer| &layer.files).any(|file| !sha(&file.sha256)) {
                return Err(Code::Usage.error("release file has no SHA-256"));
            }
            if release.product != product.product || release.id() != *id || product.document.is_none() {
                return Err(Code::VerifyFailed.error("commit release identity or desired pointer differs"));
            }
        }
    }
    if bundle.expected.len() != names.len() {
        return Err(Code::Usage.error("commit has an unexpected pointer key"));
    }
    Ok(())
}

/// This process owns every remote write and waits for its children before releasing the lock.
fn execute(directory: &Path, digest: &str, store: &Store, remote: &Remote, wait: Wait) -> Result<Committed, Error> {
    let bytes = std::fs::read(directory.join("bundle.json")).map_err(|e| e.to_string())?;
    if sha256_hex(&bytes) != digest {
        return Err(Code::VerifyFailed.error("commit bundle checksum differs"));
    }
    let bundle: Bundle = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    validate(&bundle)?;
    if bundle.bucket != remote.describe() {
        return Err(Code::Blocked.error("commit owner names another bucket"));
    }
    let Remote::Bucket(bucket) = remote else {
        return Err(Code::Blocked.error("commit needs R2 credentials on its owner"));
    };
    let mut owner = Owner::open(&store.root().join("commits"), &bundle.run, &bytes)?;
    let result_path = store.root().join("commits").join(format!("{}.result", bundle.run));
    if owner.finished() || result_path.exists() {
        let result = serde_json::from_slice(&std::fs::read(result_path).map_err(|e| e.to_string())?)
            .map_err(|e| Code::Failed.error(e.to_string()))?;
        owner.finish()?;
        return Ok(result);
    }
    for (key, expected) in &bundle.expected {
        if remote.get(key)?.as_deref().map(sha256_hex) != *expected {
            return Err(Code::PlanOutdated.error(format!("{key} changed before the commit lock; plan again")));
        }
    }
    for product in &bundle.next.products {
        if product.product == "planner" && product.release.is_some() && !same_pointer(remote, product)? {
            return Err(Code::Blocked.error("planner publication needs the service activation and retirement owner"));
        }
    }
    let payload = Store::at(directory);
    for (_, _, release) in bundle.next.releases() {
        release.write(&payload)?;
    }
    let scratch = Scratch::new()?;
    let files = apply_cli::files(&payload, &scratch, &bundle.next)?;
    let listed = bundle.next.list(remote)?;
    let mut run = Run::attach(store, &bundle.run, &bundle.journal)?;
    let result = (|| {
        run.record(&Event::Phase { phase: Phase::Upload })?;
        let uploaded = apply_cli::upload(bucket, &listed, &files, &mut run, &mut owner)?;
        run.record(&Event::Phase { phase: Phase::Switch })?;
        let mut switched = Vec::new();
        for product in &bundle.next.products {
            let Some((id, _)) = &product.release else { continue };
            let key = format!("{}/catalog.json", product.prefix);
            let mut document =
                product.document.clone().ok_or_else(|| Code::VerifyFailed.error("missing desired pointer"))?;
            if same_pointer(remote, product)? {
                continue;
            }
            document.insert("release".into(), id.clone().into());
            document.insert("applied".into(), crate::date::timestamp(crate::date::now()).into());
            let body = serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?;
            let file = apply_cli::write(&scratch, &key, &body)?;
            owner.mutate(
                &mut run,
                Intent {
                    mutation: Publication::Switched { product: product.product.clone(), release: id.clone() },
                    expected: bundle.expected[&key].clone(),
                    desired: Some(sha256_hex(&body)),
                },
                || {
                    bucket.put(
                        &file,
                        &key,
                        &Upload {
                            cache_control: Some("public, max-age=60, must-revalidate"),
                            content_type: Some("application/json"),
                            immutable: false,
                        },
                    )
                },
            )?;
            bucket.verify(&file, &key).map_err(|e| Code::VerifyFailed.error(e))?;
            switched.push(BuiltRelease { product: product.product.clone(), id: id.clone() });
        }
        let products: Vec<_> =
            bundle.next.products.iter().map(|product| (product.product.as_str(), product.prefix.as_str())).collect();
        let switch = (!switched.is_empty()).then(Instant::now);
        let removed = apply_cli::remove(
            bucket,
            (remote, &products, &bundle.sources, store),
            crate::date::now(),
            switch,
            wait,
            &mut run,
            &mut owner,
        )?;
        Ok(Committed { uploaded, switched, removed })
    })();
    let result = super::api::finish_run(run, result, None).map_err(|mut error| {
        if owner.unknown() {
            error.fix = format!("Commit {} has an unknown remote outcome. Inspect its durable intent; do not retry or clear it from a later read alone.", bundle.run);
        }
        error
    })?;
    durable(&result_path, &serde_json::to_vec(&result).map_err(|e| e.to_string())?)?;
    owner.finish()?;
    for name in ["objects", "releases"] {
        let path = directory.join(name);
        if path.exists() {
            if let Err(error) = std::fs::remove_dir_all(path) {
                eprintln!("commit payload cleanup: {error}");
            }
        }
    }
    Ok(result)
}

fn same_pointer(remote: &Remote, product: &crate::live::LiveProduct) -> Result<bool, Error> {
    let Some((id, _)) = &product.release else {
        return Ok(true);
    };
    let key = format!("{}/catalog.json", product.prefix);
    let mut observed = remote
        .get(&key)?
        .map(|bytes| {
            serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&bytes).map_err(|e| e.to_string())
        })
        .transpose()?;
    if let Some(observed) = &mut observed {
        observed.remove("applied");
    }
    let mut wanted = product.document.clone().ok_or_else(|| Code::VerifyFailed.error("missing desired pointer"))?;
    wanted.insert("release".into(), id.clone().into());
    Ok(observed == Some(wanted))
}

#[cfg(not(test))]
pub(super) fn submit(directory: &Path, digest: &str, run: &str, store: &Store) -> Result<Committed, Error> {
    let host = std::env::var("OBC_COMMIT_HOST")
        .map_err(|_| Code::Blocked.error("set OBC_COMMIT_HOST to the configured VPS, or local on that VPS"))?;
    let worker = "/opt/obc-data/bin/obc-data-plumbing";
    crate::engine::runs::check_id(run)?;
    if !sha(digest) {
        return Err(Code::Usage.error("commit bundle digest is not SHA-256"));
    }
    let incoming = format!("/var/lib/obc-data/incoming/{run}/{digest}");
    if host == "local" {
        let output = Command::new(worker)
            .args(["commit", directory.to_str().ok_or_else(|| Code::Usage.error("bundle path is not UTF-8"))?, digest])
            .output()
            .map_err(|e| e.to_string())?;
        return response(output, store, run);
    }
    if host.is_empty()
        || !host.bytes().all(|c| c.is_ascii_alphanumeric() || b".-_@".contains(&c))
        || host.starts_with('-')
    {
        return Err(Code::Usage.error("OBC_COMMIT_HOST is not an SSH host"));
    }
    let prepared =
        Command::new("ssh").args(["-T", &host, "mkdir", "-p", "--", &incoming]).output().map_err(|e| e.to_string())?;
    if !prepared.status.success() {
        return Err(Code::Failed.error("commit transfer directory could not be made; no publication owner started"));
    }
    let copied = Command::new("rsync")
        .args(["-r", "--", &format!("{}/", directory.display()), &format!("{host}:{incoming}/")])
        .output()
        .map_err(|e| e.to_string())?;
    if !copied.status.success() {
        return Err(Code::Failed.error("commit bundle transfer failed; no publication owner started"));
    }
    let output = Command::new("ssh")
        .args(["-T", &host, "nohup", worker, "commit", &incoming, digest])
        .output()
        .map_err(|e| e.to_string())?;
    response(output, store, run).map_err(|mut error| {
        if error.message.contains("failed or disconnected") {
            error.fix =
                format!("Query commit {run} on the VPS. Do not retry a remote mutation with an unknown outcome.");
        }
        error.run = Some(run.into());
        error
    })
}

#[cfg(not(test))]
fn response(output: std::process::Output, store: &Store, run: &str) -> Result<Committed, Error> {
    match serde_json::from_slice(&output.stdout) {
        Ok(Reply::Done { result, journal }) if output.status.success() => {
            Run::mirror(store, run, &journal)?;
            Ok(result)
        }
        Ok(Reply::Failed { error, journal }) => {
            if let Some(journal) = journal {
                Run::mirror(store, run, &journal)?;
            }
            Err(error)
        }
        _ => Err(Code::Blocked.error("commit worker failed or disconnected; inspect its durable state and run")),
    }
}

/// Plumbing commands run without a producer checkout or the GPL step graph.
pub fn main(args: &[String]) -> Result<u8, String> {
    let store = Store::at("/var/lib/obc-data/store");
    match args {
        [command, run] if command == "commit-status" => {
            crate::engine::runs::check_id(run)?;
            let path = store.root().join("commits").join(format!("{run}.json"));
            print!("{}", std::fs::read_to_string(path).map_err(|e| e.to_string())?);
            Ok(0)
        }
        [command, directory, digest] if command == "commit" => {
            if !cfg!(target_os = "linux") {
                return Err("the publication owner runs on the configured Linux VPS".into());
            }
            #[cfg(unix)]
            // SAFETY: ignore only hangup. The owner waits for each lock-inheriting child.
            unsafe {
                libc::signal(libc::SIGHUP, libc::SIG_IGN);
            }
            let remote = Remote::from_env()?;
            let result = execute(Path::new(directory), digest, &store, &remote, WAIT);
            let journal = std::fs::read(Path::new(directory).join("bundle.json"))
                .ok()
                .filter(|bytes| sha256_hex(bytes) == *digest)
                .and_then(|bytes| serde_json::from_slice::<Bundle>(&bytes).ok())
                .and_then(|bundle| crate::engine::runs::events(&store, &bundle.run).ok());
            let reply = match result {
                Ok(result) => Reply::Done { result, journal: journal.ok_or("commit journal is missing")? },
                Err(error) => Reply::Failed { error, journal },
            };
            let code = match &reply {
                Reply::Done { .. } => 0,
                Reply::Failed { error, .. } => error.code.exit(),
            };
            println!("{}", serde_json::to_string(&reply).map_err(|e| e.to_string())?);
            Ok(code)
        }
        _ => Err("use commit BUNDLE SHA256 or commit-status RUN".into()),
    }
}

#[cfg(test)]
pub(super) fn execute_for_test(
    directory: &Path,
    digest: &str,
    store: &Store,
    remote: &Remote,
    wait: Wait,
) -> Result<Committed, Error> {
    execute(directory, digest, store, remote, wait)
}
