//! The product-free final writer, with the original operation journal.

pub(super) mod lifetime;

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
    services: Vec<crate::vps::Candidate>,
    previous_services: Vec<crate::vps::Candidate>,
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
pub(super) enum Reply {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn pack(
    directory: &Path,
    store: &Store,
    run: &Run,
    expected: BTreeMap<String, Option<String>>,
    mut next: Live,
    sources: Vec<Source>,
    remote: &Remote,
    services: (Vec<crate::vps::Candidate>, Vec<crate::vps::Candidate>),
) -> Result<String, Error> {
    next.products.retain(|product| expected.contains_key(&format!("{}/catalog.json", product.prefix)));
    let (mut services, mut previous_services) = services;
    for candidate in &mut previous_services {
        candidate.source = std::path::PathBuf::from("previous-services").join(candidate.service.name());
    }
    for candidate in &mut services {
        candidate.source = std::path::PathBuf::from("services").join(candidate.service.name());
    }
    let bundle = Bundle {
        run: run.id().into(),
        bucket: remote.describe().into(),
        journal: crate::engine::runs::events(store, run.id())?,
        expected,
        next,
        sources,
        services,
        previous_services,
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
    let planner = bundle.next.products.iter().find(|product| product.product == "planner" && product.release.is_some());
    if let Some(planner) = planner {
        let release = &planner.release.as_ref().unwrap().1;
        if bundle.services.len() != 3
            || bundle.services.iter().map(|candidate| candidate.service).collect::<BTreeSet<_>>().len() != 3
        {
            return Err(Code::Blocked.error("planner needs all three prepared service views"));
        }
        let document =
            planner.document.as_ref().ok_or_else(|| Code::VerifyFailed.error("planner has no desired document"))?;
        let origins: crate::vps::Origins = serde_json::from_value(
            document.get("origins").cloned().ok_or_else(|| Code::VerifyFailed.error("planner has no origins"))?,
        )
        .map_err(|e| e.to_string())?;
        origins.check()?;
        for candidate in &bundle.services {
            if candidate.source != std::path::PathBuf::from("services").join(candidate.service.name())
                || !release.named.contains(&candidate.release)
                || !release.named.contains(&candidate.runtime)
                || candidate.release.path != "release.json"
                || candidate.runtime.path != format!("runtime/{}.json", candidate.service.name())
                || candidate.site_origin != origins.site_origin
                || candidate.api_origin != origins.api_origin
                || candidate.objects_url != origins.objects_url()
                || document["active"]["services"][candidate.service.name()] != candidate.id
            {
                return Err(
                    Code::VerifyFailed.error("prepared service differs from its release or desired publication")
                );
            }
        }
    } else if !bundle.services.is_empty() {
        return Err(Code::Usage.error("service views have no planner release"));
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
    let mut observed = BTreeMap::new();
    for (key, expected) in &bundle.expected {
        let body = remote.get(key)?;
        if body.as_deref().map(sha256_hex) != *expected {
            return Err(Code::PlanOutdated.error(format!("{key} changed before the commit lock; plan again")));
        }
        let document = body
            .map(|body| {
                serde_json::from_slice::<serde_json::Map<String, serde_json::Value>>(&body).map_err(|e| e.to_string())
            })
            .transpose()?;
        observed.insert(key.clone(), document);
    }
    let planner = bundle.next.products.iter().find(|product| product.product == "planner" && product.release.is_some());
    let current_document = observed
        .get("planner/catalog.json")
        .and_then(Option::as_ref)
        .filter(|document| document.contains_key("release"));
    let current = if planner.is_some() { crate::vps::current(current_document)? } else { Vec::new() };
    let approved = if let Some(planner) = planner {
        let mut wanted =
            planner.document.clone().ok_or_else(|| Code::VerifyFailed.error("planner has no desired pointer"))?;
        let chosen = crate::vps::document(&mut wanted, current_document)?;
        if planner.document.as_ref() != Some(&wanted) {
            return Err(Code::PlanOutdated.error("service slot choices differ from the approved pointer"));
        }
        let original = Live::read_products(remote, &[("planner", "planner")], &bundle.sources, store)?;
        if original.products.first().map(|product| &product.observed) != bundle.expected.get("planner/catalog.json") {
            return Err(Code::PlanOutdated.error("planner changed while reading original service metadata"));
        }
        let release =
            original.products.first().and_then(|product| product.release.as_ref()).map(|(_, release)| release);
        if current.len() != bundle.previous_services.len() {
            return Err(Code::Blocked.error("current service metadata is incomplete"));
        }
        for candidate in &bundle.previous_services {
            let release =
                release.ok_or_else(|| Code::Blocked.error("previous service metadata has no original live release"))?;
            if candidate.source != std::path::PathBuf::from("previous-services").join(candidate.service.name())
                || !release.named.contains(&candidate.release)
                || !release.named.contains(&candidate.runtime)
                || candidate.release.path != "release.json"
                || candidate.runtime.path != format!("runtime/{}.json", candidate.service.name())
            {
                return Err(Code::VerifyFailed.error("previous service view differs from exact observed live metadata"));
            }
            let origins: crate::vps::Origins = serde_json::from_value(
                current_document
                    .and_then(|document| document.get("origins"))
                    .cloned()
                    .ok_or_else(|| Code::VerifyFailed.error("original planner has no publication origins"))?,
            )
            .map_err(|e| Code::VerifyFailed.error(e.to_string()))?;
            origins.check()?;
            if candidate.api_origin != origins.api_origin
                || candidate.site_origin != origins.site_origin
                || candidate.objects_url != origins.objects_url()
            {
                return Err(Code::VerifyFailed.error("previous service origins differ from exact observed live"));
            }
        }
        crate::vps::commit::stages(&bundle.previous_services, &current)?;
        chosen
    } else {
        if !bundle.previous_services.is_empty() {
            return Err(Code::Usage.error("previous service views have no planner publication"));
        }
        Vec::new()
    };
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
        let candidates = |views: &[crate::vps::Candidate]| {
            views
                .iter()
                .map(|candidate| {
                    let mut candidate = candidate.clone();
                    candidate.source = directory.join(&candidate.source);
                    candidate
                })
                .collect::<Vec<_>>()
        };
        let desired = candidates(&bundle.services);
        let previous = candidates(&bundle.previous_services);
        let mut service_owner = desired
            .iter()
            .find(|candidate| candidate.service == crate::vps::Service::Downloads)
            .map(|downloads| crate::vps::local::Local::new(&payload, bucket, downloads))
            .transpose()
            .map_err(|e| Code::Blocked.error(e))?;
        if let Some(backend) = &mut service_owner {
            let mut guarded = crate::vps::commit::Guarded { backend, owner: &mut owner, run: &mut run };
            crate::vps::commit::activate(&mut guarded, &desired, &previous, &current, &approved, || {
                std::thread::sleep(wait.pointer + wait.clock)
            })?;
        }
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
        if let Some(backend) = &mut service_owner {
            use crate::vps::Vps;
            if current.iter().any(|unit| !approved.contains(unit)) {
                let switched = switch
                    .ok_or_else(|| Code::Blocked.error("service retirement needs an acknowledged pointer switch"))?;
                run.record(&Event::Phase { phase: Phase::Wait })?;
                std::thread::sleep((wait.pointer + wait.clock).saturating_sub(switched.elapsed()));
                run.record(&Event::Phase { phase: Phase::Cleanup })?;
            }
            crate::vps::commit::Guarded { backend, owner: &mut owner, run: &mut run }.retire(&current, &approved)?;
        }
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
pub(super) fn host(host: &str) -> Result<(), Error> {
    if host.is_empty()
        || !host.bytes().all(|c| c.is_ascii_alphanumeric() || b".-_@".contains(&c))
        || host.starts_with('-')
    {
        return Err(Code::Usage.error("OBC_COMMIT_HOST is not an SSH host"));
    }
    Ok(())
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
    self::host(&host)?;
    if host == "local" {
        std::fs::create_dir_all(&incoming).map_err(|e| e.to_string())?;
    } else {
        let prepared = Command::new("ssh")
            .args(["-T", &host, "mkdir", "-p", "--", &incoming])
            .output()
            .map_err(|e| e.to_string())?;
        if !prepared.status.success() {
            return Err(Code::Failed.error("commit transfer directory could not be made; no publication owner started"));
        }
    }
    let target = if host == "local" { format!("{incoming}/") } else { format!("{host}:{incoming}/") };
    let copied = Command::new("rsync")
        .args(["-r", "--", &format!("{}/", directory.display()), &target])
        .output()
        .map_err(|e| e.to_string())?;
    if !copied.status.success() {
        return Err(Code::Failed.error("commit bundle transfer failed; no publication owner started"));
    }
    // Persist the handoff before admission. Transport failure cannot revoke a delayed dispatch.
    super::operation_cli::handoff(store, run, &host, digest)?;
    let mut command = if host == "local" {
        Command::new(worker)
    } else {
        let mut command = Command::new("ssh");
        command.args(["-T", &host, worker]);
        command
    };
    let admitted = command.args(["commit-start", &incoming, digest]).output().map_err(|e| e.to_string())?;
    if !admitted.status.success() {
        return Err(Code::Blocked
            .error("commit service admission failed or is uncertain")
            .fix("Inspect the bound owner status. A delayed admission is still possible.")
            .with_run(run));
    }
    loop {
        let observed = lifetime::query(&host, run, digest)?;
        let pending = observed.state.as_ref().is_some_and(|state| state.pending.is_some());
        if pending && observed.reply.is_some() {
            return Err(Code::Blocked
                .error("commit has an unknown mutation outcome")
                .fix("Inspect its durable owner intent. A later read alone cannot clear it.")
                .with_run(run));
        }
        if observed.reply.is_some() {
            super::operation_cli::checked_owner(store, run, &host, digest, &observed)?;
        }
        if let Some(reply) = observed.reply {
            return terminal(reply, store, run, &host, digest);
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}
#[cfg(not(test))]
fn terminal(reply: Reply, store: &Store, run: &str, host: &str, digest: &str) -> Result<Committed, Error> {
    let journal = lifetime::journal(&reply).ok_or_else(|| Code::Blocked.error("terminal owner has no run journal"))?;
    Run::mirror(store, run, journal)?;
    super::operation_cli::resolved(store, run, host, digest, matches!(reply, Reply::Done { .. }), &reply)?;
    match reply {
        Reply::Done { result, .. } => Ok(result),
        Reply::Failed { error, .. } => Err(error),
    }
}

/// Plumbing commands run without a producer checkout or the GPL step graph.
pub fn main(args: &[String]) -> Result<u8, String> {
    let store = Store::at("/var/lib/obc-data/store");
    match args {
        [command, run, digest] if command == "commit-status" => {
            println!("{}", serde_json::to_string(&lifetime::observe(&store, run, digest)?).map_err(|e| e.to_string())?);
            Ok(0)
        }
        [command, directory, digest] if command == "commit-start" => {
            lifetime::admit(&store, Path::new(directory), digest)?;
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
            let bytes = std::fs::read(Path::new(directory).join("bundle.json")).map_err(|e| e.to_string())?;
            if sha256_hex(&bytes) != *digest {
                return Err("commit bundle checksum differs".into());
            }
            let bundle: Bundle = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            validate(&bundle).map_err(|error| error.message)?;
            let result = Remote::from_env()
                .map_err(|e| Code::Blocked.error(e))
                .and_then(|remote| execute(Path::new(directory), digest, &store, &remote, WAIT));
            if let Err(error) = &result {
                let events =
                    crate::engine::runs::events(&store, &bundle.run).unwrap_or_else(|_| bundle.journal.clone());
                if !matches!(events.last(), Some(Event::Finished { .. })) {
                    let run = Run::attach(&store, &bundle.run, &events)?;
                    run.finish(Some(&error.message))?;
                }
            }
            let journal = crate::engine::runs::events(&store, &bundle.run).ok();
            let reply = match result {
                Ok(result) => Reply::Done { result, journal: journal.ok_or("commit journal is missing")? },
                Err(error) => Reply::Failed { error, journal },
            };
            if lifetime::observe(&store, &bundle.run, digest)?.state.is_some() {
                lifetime::persist_reply(&store, &bundle.run, &reply)?;
            }
            let code = match &reply {
                Reply::Done { .. } => 0,
                Reply::Failed { error, .. } => error.code.exit(),
            };
            println!("{}", serde_json::to_string(&reply).map_err(|e| e.to_string())?);
            Ok(code)
        }
        _ => Err("use commit-start BUNDLE SHA256, commit BUNDLE SHA256, or commit-status RUN SHA256".into()),
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
