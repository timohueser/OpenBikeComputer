//! `obc data apply live`: build the plan of live and check its releases, upload what R2 lacks,
//! switch the pointer of each product that the plan changes, and then remove from R2 what no live
//! release uses. Until the pointers switch, live does not change.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::build_cli::{self, Applying, BuildArgs, Built, BuiltRelease, EnvPlan};
use super::{bytes, confirm, registry, Code, Error};
use crate::date;
use crate::engine::runs::{Event, Phase, Publication, Run};
use crate::fetch::http::Http;
use crate::live::{Live, Remote, INPUTS};
use crate::product::Product;
use crate::r2::{Bucket, Object, Scratch, Upload};
use crate::sources::Source;
use crate::store::{hash_file, write_atomic, Store};

/// How long the files of the releases before a switch stay on R2: a client that read an old
/// pointer finishes its downloads.
#[derive(Clone, Copy)]
pub(super) struct Wait {
    /// After the switch of this apply, and after the time of the newest pointer on R2.
    pub(super) pointer: Duration,
    /// More after the time of a pointer on R2, whose clock is not the clock of this machine.
    pub(super) clock: Duration,
}

pub(super) const WAIT: Wait = Wait { pointer: Duration::from_secs(600), clock: Duration::from_secs(120) };

/// Seconds before the start of an apply in which a key can be one of another apply.
const YOUNG: u64 = 300;

/// A pointer is at most this old in a cache, so a switch reaches every client soon.
const JSON: &str = "application/json";
const IMMUTABLE: Upload<'static> =
    Upload { cache_control: Some("public, max-age=31536000, immutable"), content_type: None, immutable: true };

#[derive(Args)]
pub struct ApplyArgs {
    /// The environment. Only `live` applies.
    pub(super) env: String,
    /// Apply this output of `plan live --json`. Exit status 3 when live or the plan of now differs.
    /// Without a terminal, it is the consent.
    #[arg(long)]
    pub(super) plan: Option<PathBuf>,
    /// Do not ask.
    #[arg(long)]
    pub(super) yes: bool,
}

/// What an apply did.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct Applied {
    pub run: String,
    /// The build of the plan; `null` when live had every change.
    pub built: Option<Built>,
    /// The keys that it uploaded.
    pub uploaded: Vec<String>,
    /// The release of each product whose pointer it switched.
    pub switched: Vec<BuiltRelease>,
    /// The keys that it removed: no live release used them.
    pub removed: Vec<Object>,
    pub approval: crate::approval::Outcome,
}

pub fn apply(root: &Path, products: &[&dyn Product], args: ApplyArgs, json: bool) -> Result<(), Error> {
    if args.env != "live" {
        return Err(Code::Usage.error(format!("`apply {}`: only `live` applies", args.env)));
    }
    let consent = consent(&args, std::io::stdin().is_terminal())?;
    let saved = args.plan.as_deref().map(build_cli::read_plan).transpose()?;
    let ask = |plan: &EnvPlan| {
        if !json {
            build_cli::print_plan(plan);
        }
        confirm(&question(plan), consent)
    };
    let (store, remote) = (Store::open()?, super::remote()?);
    let applied = apply_live(root, &store, &Http::new(), &remote, products, saved.as_ref(), ask, WAIT)?;
    if json {
        return super::print_json(&applied);
    }
    println!("{}", applied.approval.summary());
    if applied.built.is_none() {
        println!("Live has every change.");
        return Ok(());
    }
    println!("uploaded {} keys", applied.uploaded.len());
    for switched in &applied.switched {
        println!("live {}: release {}", switched.product, &switched.id[..8]);
    }
    let removed = bytes(applied.removed.iter().map(|object| object.bytes).sum());
    println!("removed {} keys, {removed}", applied.removed.len());
    Ok(())
}

/// Whether the apply goes on without a question: with `--yes`, or with `--plan` and no terminal.
/// Without a terminal, one of them is required.
pub(super) fn consent(args: &ApplyArgs, terminal: bool) -> Result<bool, Error> {
    if !terminal && !args.yes && args.plan.is_none() {
        return Err(Code::NoTerminal.error("there is no terminal to ask in; nothing changed"));
    }
    Ok(args.yes || !terminal)
}

/// The one question before an apply.
pub(super) fn question(plan: &EnvPlan) -> String {
    let removes = bytes(plan.remove.iter().filter_map(|removal| removal.bytes).sum());
    let changes = match plan.groups.len() {
        1 => "1 change".into(),
        n => format!("{n} changes"),
    };
    format!("Apply {changes} to live? removes {removes} from R2; review auto approval")
}

/// Apply `saved`, or else the plan of now. `ask` gets the plan before anything changes. The
/// removal waits as `wait` says.
#[allow(clippy::too_many_arguments)]
fn apply_live(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: &Remote,
    products: &[&dyn Product],
    saved: Option<&EnvPlan>,
    ask: impl FnOnce(&EnvPlan) -> Result<(), Error>,
    wait: Wait,
) -> Result<Applied, Error> {
    let Remote::Bucket(_) = remote else {
        return Err(Code::Blocked.error("an apply writes R2: the OBC_R2_* variables are not set"));
    };
    committed(root)?;
    let _lock = store.try_lock("apply-live")?.ok_or_else(|| {
        Code::Usage.error("another apply of live runs on this machine").fix("Wait for it to end, then plan again.")
    })?;
    let now;
    let plan = match saved {
        Some(plan) => plan,
        None => {
            now = build_cli::plan_live(root, store, http, remote, products, &[], false)?;
            &now
        }
    };
    build_cli::complete(Some(plan))?;
    build_cli::suits(products, plan)?;
    plan.approval
        .as_ref()
        .ok_or_else(|| Code::PlanOutdated.error("saved Live plan has no approval review"))?
        .check()
        .map_err(|reason| Code::Blocked.error(reason))?;
    if let Some(blocked) = plan.blocked.iter().find(|blocked| !blocked.layers.is_empty()) {
        let reasons =
            blocked.layers.iter().map(|layer| format!("{}: {}", layer.layer, layer.reason)).collect::<Vec<_>>();
        return Err(Code::Blocked.error(format!(
            "product `{}` is incomplete: {}",
            blocked.product,
            reasons.join("; ")
        )));
    }

    let noop = plan.groups.is_empty() && plan.remove.is_empty();
    ask(plan)?;
    let mut run = super::api::start_run(store, "apply live")?;
    run.require_committed_code();
    let preparation = (|| {
        let expected = super::commit_cli::expected(plan, products)?;
        let scratch = Scratch::new()?;
        let (built, next) = stage(root, store, http, remote, products, plan, &mut run)?;
        crate::worker::check(root)?;
        let directory = scratch.0.join("bundle");
        let sources = registry(root)?.sources;
        let previous = Live::read(remote, products, &sources, store).map_err(r2_failed)?;
        if previous.products.iter().any(|product| {
            expected
                .get(&format!("{}/catalog.json", product.prefix))
                .is_some_and(|observed| observed != &product.observed)
        }) {
            return Err(Code::PlanOutdated.error("live changed while preparing service metadata"));
        }
        previous.restore_named(remote, store).map_err(r2_failed)?;
        let views = |live: &Live, folder: &str| -> Result<Vec<crate::vps::Candidate>, Error> {
            let mut services = Vec::new();
            for product in products {
                let Some(now) = live.products.iter().find(|now| now.product == product.name()) else { continue };
                let Some((_, release)) = &now.release else { continue };
                let mut candidates = product.services(root, release, store, &directory.join(folder))?;
                if !candidates.is_empty() {
                    let origins: crate::vps::Origins = serde_json::from_value(
                        now.document
                            .as_ref()
                            .and_then(|document| document.get("origins"))
                            .cloned()
                            .ok_or_else(|| Code::VerifyFailed.error("service view has no pointer origins"))?,
                    )
                    .map_err(|e| e.to_string())?;
                    origins.check()?;
                    for candidate in &mut candidates {
                        candidate.site_origin = origins.site_origin.clone();
                        candidate.api_origin = origins.api_origin.clone();
                        candidate.objects_url = origins.objects_url();
                    }
                }
                services.extend(candidates);
            }
            Ok(services)
        };
        let services = (views(&next, "services")?, views(&previous, "previous-services")?);
        build_cli::recheck_approval(root, store, http, remote, products, plan, &mut run)?;
        let approval =
            plan.approval.clone().ok_or_else(|| Code::PlanOutdated.error("saved Live plan has no approval review"))?;
        let digest =
            super::commit_cli::pack(&directory, store, &run, expected, next, sources, remote, services, approval)?;
        run.sync()?;
        Ok((built, scratch, directory, digest))
    })();
    let (built, _scratch, directory, digest) = match preparation {
        Ok(prepared) => prepared,
        Err(error) => return super::api::finish_run(run, Err(error), None),
    };
    let id = run.id().to_string();
    drop(run);
    #[cfg(test)]
    let committed = super::commit_cli::execute_for_test(&directory, &digest, store, remote, wait)?;
    #[cfg(not(test))]
    let committed = {
        let _ = wait;
        super::commit_cli::submit(&directory, &digest, &id, store).map_err(|mut error| {
            error.run = Some(id.clone());
            error
        })?
    };
    let mut applied = Applied { run: id, built: (!noop).then_some(built), ..Applied::default() };
    committed.apply(&mut applied);
    Ok(applied)
}

/// Remove what no live release uses once no client can still read an older pointer: `wait` after
/// the newest pointer on R2, and after `switch`, the switch of this apply. Each round reads live
/// and lists R2 again, and the removal takes the leftovers of the last round only: another apply
/// can switch during the wait.
pub(super) fn remove(
    bucket: &Bucket,
    reader: (&Remote, &[(&str, &str)], &[Source], &Store),
    start: u64,
    switch: Option<Instant>,
    wait: Wait,
    run: &mut Run,
    owner: &mut crate::commit::Owner,
) -> Result<Vec<Object>, Error> {
    let since = |instant: Instant| wait.pointer.saturating_sub(instant.elapsed());
    let mut unknown = None;
    loop {
        let (leftovers, newest) = leftovers(reader, start)?;
        if leftovers.is_empty() {
            return Ok(leftovers);
        }
        let by_clock = match newest {
            Some(newest) => {
                let until = newest + (wait.pointer + wait.clock).as_secs();
                Duration::from_secs(until.saturating_sub(date::now()))
            }
            // No time of a pointer reads: the whole wait, from the first round.
            None => since(*unknown.get_or_insert_with(Instant::now)),
        };
        let left = by_clock.max(switch.map_or(Duration::ZERO, since));
        if left.is_zero() {
            run.record(&Event::Phase { phase: Phase::Cleanup })?;
            delete(bucket, &leftovers, "obc data apply live: no live release uses it", run, owner)?;
            return Ok(leftovers);
        }
        eprintln!("obc data: the old releases stay {} s more for clients that read an old pointer", left.as_secs());
        run.record(&Event::Phase { phase: Phase::Wait })?;
        std::thread::sleep(left);
    }
}

/// Build `plan`, check its releases and upload what R2 lacks. Live does not change.
#[allow(clippy::too_many_arguments)]
fn stage(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: &Remote,
    products: &[&dyn Product],
    plan: &EnvPlan,
    run: &mut Run,
) -> Result<(Built, Live), Error> {
    let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: None, moves: Vec::new() };
    let (built, applying) = build_cli::build_env(root, store, http, Some(remote), products, &args, Some(plan), run)?;
    let Applying { next } = applying.expect("a build of live gives what an apply changes");
    run.record(&Event::Phase { phase: Phase::Verify })?;
    verify_products(root, products, store, &next)?;
    Ok((built, next))
}

fn r2_failed(message: String) -> Error {
    Code::R2Failed.error(message)
}

/// Write `bytes` for `key` into `scratch`, and give the path.
pub(super) fn write(scratch: &Scratch, key: &str, bytes: &[u8]) -> Result<PathBuf, Error> {
    let path = scratch.0.join(key);
    write_atomic(&path, bytes)?;
    Ok(path)
}

/// Refuse an apply while `data/` has changes that git does not have: live builds from a committed
/// `data/`. `data/env/local.toml` is never in git.
fn committed(root: &Path) -> Result<(), Error> {
    let args = ["status", "--porcelain", "--untracked-files=all", "--", "data", ":(exclude)data/env/local.toml"];
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .map_err(|e| Code::Failed.error(format!("git: {e}")))?;
    if !out.status.success() {
        return Err(Code::Failed.error(format!("git status: {}", String::from_utf8_lossy(&out.stderr).trim())));
    }
    let changed = String::from_utf8_lossy(&out.stdout);
    if changed.trim().is_empty() {
        return Ok(());
    }
    let paths: Vec<&str> = changed.lines().map(|line| line.get(3..).unwrap_or(line)).collect();
    Err(Code::Usage
        .error(format!("data/ has changes that are not committed: {}", paths.join(", ")))
        .fix("Commit data/ first: an apply builds live from the committed data/."))
}

/// Verify each complete product before publication, including unchanged releases.
fn verify_products(root: &Path, products: &[&dyn Product], store: &Store, next: &Live) -> Result<(), Error> {
    for (product, next) in products.iter().zip(&next.products) {
        let Some((id, release)) = next.release.as_ref() else {
            continue;
        };
        let name = product.name();
        let failed = |e: String| {
            Code::VerifyFailed
                .error(format!("release {} of `{name}`: {e}; nothing changed", &id[..8]))
                .fix(format!("Correct the steps of product `{name}`, then plan again."))
        };
        product.verify(root, None, release, store).map_err(failed)?;
        if next.document.is_none() {
            return Err(failed("release has no desired pointer".into()));
        }
    }
    Ok(())
}

/// A key that live needs, and the file that holds its bytes.
pub(super) struct File {
    pub(super) key: String,
    pub(super) path: PathBuf,
    /// The size of the key on R2; `None` when any object at the key serves.
    pub(super) size: Option<u64>,
    /// The SHA-256 of the file, when the key names it.
    pub(super) sha256: Option<String>,
    pub(super) upload: Upload<'static>,
}

/// Every key of `next` but the pointers: the manifests and objects of its releases, the files that
/// a client finds by name, and the input copies.
pub(super) fn files(store: &Store, scratch: &Scratch, next: &Live) -> Result<Vec<File>, Error> {
    let mut files = Vec::new();
    for (prefix, id, release) in next.releases() {
        files.push(File {
            key: format!("{prefix}/releases/{id}.json"),
            path: store.release(&release.product, id),
            size: Some(release.canonical().len() as u64),
            sha256: Some(id.into()),
            upload: Upload { content_type: Some(JSON), ..IMMUTABLE },
        });
        files.extend(release.named.iter().map(|file| File {
            key: format!("{prefix}/releases/{id}/{}", file.path),
            path: store.object(&file.sha256),
            size: Some(file.size),
            sha256: Some(file.sha256.clone()),
            upload: IMMUTABLE,
        }));
        files.extend(release.objects().into_iter().map(|(sha256, size)| File {
            key: format!("{prefix}/objects/{sha256}"),
            path: store.object(sha256),
            size: Some(size),
            sha256: Some(sha256.into()),
            upload: IMMUTABLE,
        }));
    }
    for (read, record) in &next.inputs {
        let (source, version) = (&read.source, &read.version);
        let record = record.as_ref().ok_or_else(|| {
            Code::Failed
                .error(format!("neither R2 nor the store has the record of the input copy {source}@{version}"))
                .fix(format!("Fetch it with `obc data fetch {source}@{version}`, or plan with `--move {source}`."))
        })?;
        let key = read.path();
        let bytes = record.canonical();
        let upload = Upload { content_type: Some(JSON), ..IMMUTABLE };
        files.push(File {
            path: write(scratch, &key, &bytes)?,
            key,
            size: Some(bytes.len() as u64),
            sha256: Some(crate::store::sha256_hex(&bytes)),
            upload,
        });
        files.extend(record.files.iter().map(|file| File {
            key: format!("{INPUTS}/objects/{}", file.sha256),
            path: store.object(&file.sha256),
            size: Some(file.size),
            sha256: Some(file.sha256.clone()),
            upload: IMMUTABLE,
        }));
    }
    Ok(files)
}

/// Upload each of `files` that `listed` lacks or holds with another size, and give their keys. A
/// key with another size goes first: an immutable upload never replaces. Each file is checked
/// against its SHA-256 before R2 changes, and each key after its upload.
pub(super) fn upload(
    bucket: &Bucket,
    listed: &[Object],
    files: &[File],
    run: &mut Run,
    owner: &mut crate::commit::Owner,
) -> Result<Vec<String>, Error> {
    for file in files.iter().filter(|f| {
        let named = f.key.split_once("/releases/").is_some_and(|(_, path)| path.contains('/'));
        (f.key.starts_with(&format!("{INPUTS}/records/")) || named) && listed.iter().any(|object| object.key == f.key)
    }) {
        let bytes = bucket
            .read(&file.key)
            .map_err(r2_failed)?
            .ok_or_else(|| Code::R2Failed.error(format!("{} disappeared", file.key)))?;
        if file.sha256.as_deref() != Some(crate::store::sha256_hex(&bytes).as_str()) {
            return Err(Code::VerifyFailed
                .error(format!("{} already holds different immutable metadata; nothing changed", file.key)));
        }
    }
    let listed: BTreeMap<&str, &Object> = listed.iter().map(|object| (object.key.as_str(), object)).collect();
    let (mut missing, mut wrong) = (BTreeMap::new(), Vec::new());
    for file in files {
        match listed.get(file.key.as_str()) {
            Some(object) if file.size.is_none_or(|size| size == object.bytes) => continue,
            Some(object) => wrong.push((*object).clone()),
            None => {}
        }
        missing.insert(file.key.as_str(), file);
    }
    if missing.is_empty() {
        return Ok(Vec::new());
    }
    for file in missing.values() {
        let Some(sha256) = &file.sha256 else { continue };
        let (found, size) = hash_file(&file.path).map_err(|e| Code::Failed.error(format!("{}: {e}", file.key)))?;
        if (&found, Some(size)) != (sha256, file.size) {
            let message = format!("{} holds other bytes than {}; nothing changed", file.path.display(), file.key);
            return Err(Code::VerifyFailed.error(message).fix("Remove that file from the store, then plan again."));
        }
    }
    if !wrong.is_empty() {
        delete(bucket, &wrong, "obc data apply live: the key holds another size than live needs", run, owner)?;
    }
    for file in missing.values() {
        owner
            .mutate(
                run,
                crate::commit::Intent {
                    mutation: Publication::Uploaded { key: file.key.clone() },
                    expected: None,
                    desired: file.sha256.clone(),
                },
                || bucket.put(&file.path, &file.key, &file.upload),
            )
            .map_err(r2_failed)?;
    }
    let keys: Vec<String> = missing.keys().map(|key| key.to_string()).collect();
    let found = bucket.stat(&keys).map_err(r2_failed)?;
    for file in missing.values() {
        let size = found.get(&file.key).map(|object| object.bytes);
        if size.is_none() || file.size.is_some_and(|expected| Some(expected) != size) {
            return Err(Code::VerifyFailed.error(format!("{}: R2 does not hold the uploaded file", file.key)));
        }
    }
    Ok(keys)
}

/// What an apply removes, from one read of live and one listing: the keys under the prefixes of
/// live that no live release uses and that R2 had [`YOUNG`] before `start`, and the time of the
/// newest pointer, when one reads. Drift is an error: a listing that lacks a key of live is no
/// ground for a removal.
fn delete(
    bucket: &Bucket,
    objects: &[Object],
    reason: &str,
    run: &mut Run,
    owner: &mut crate::commit::Owner,
) -> Result<(), Error> {
    for object in objects {
        owner
            .mutate(
                run,
                crate::commit::Intent {
                    mutation: Publication::Removed { key: object.key.clone(), bytes: object.bytes },
                    expected: Some(format!("{}:{}", object.modified, object.bytes)),
                    desired: None,
                },
                || bucket.delete(std::slice::from_ref(object), reason),
            )
            .map_err(r2_failed)?;
    }
    Ok(())
}

pub(super) fn leftovers(
    (remote, products, sources, store): (&Remote, &[(&str, &str)], &[Source], &Store),
    start: u64,
) -> Result<(Vec<Object>, Option<u64>), Error> {
    let live = Live::read_products(remote, products, sources, store).map_err(r2_failed)?;
    let mut listed = Vec::new();
    for prefix in live.swept() {
        listed.extend(remote.list(&prefix).map_err(r2_failed)?);
    }
    let check = live.check(&listed);
    if let Some(drift) = check.drift.first() {
        let message = format!("R2 lacks `{}` of live, so nothing was removed", drift.key);
        return Err(Code::R2Failed.error(message).fix("Run `obc data apply live` again: it uploads what R2 lacks."));
    }
    let pointers: Vec<String> = live.releases().map(|(prefix, _, _)| format!("{prefix}/catalog.json")).collect();
    let switched = listed.iter().filter(|object| pointers.contains(&object.key));
    let newest = switched.filter_map(|object| date::seconds(&object.modified)).max();
    // A key that another apply uploads now, but has not switched to yet, is young.
    let older = |object: &Object| date::seconds(&object.modified).is_some_and(|modified| modified + YOUNG < start);
    Ok((check.leftovers.into_iter().filter(older).collect(), newest))
}

#[cfg(test)]
mod tests {
    use std::process::Command;
    use std::time::SystemTime;

    use super::*;
    use crate::cli::build_cli::tests::{upstream, Versioned, SOURCES};
    use crate::engine::tests::{fixture, write, Fixture, JOIN};
    use crate::env::Env;
    use crate::product::{Pointer, PointerFn, Unplanned};
    use crate::regions::Regions;
    use crate::sources::parse_sources;
    use crate::store::sha256_hex;

    const NO_WAIT: Wait = Wait { pointer: Duration::ZERO, clock: Duration::ZERO };

    /// A committed repository, `head@2020-01-01` in the store, and a local bucket with a
    /// firmware file and the removal log.
    fn repository(name: &str) -> (Fixture, Remote) {
        let fixture = fixture(name);
        let root = fixture.root();
        write(&root.join("data/sources.toml"), SOURCES);
        write(
            &root.join("data/regions/monaco.toml"),
            "name = \"Monaco\"\nkind = \"geofabrik\"\nareas = [\"monaco\"]\n",
        );
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        commit(&fixture);
        fixture.fetched_version("head", "2020-01-01", "head.txt", b"head\n");
        upstream(&fixture, "head", "2020-01-01");
        upstream(&fixture, "tail", "1");
        let dir = fixture.scratch.0.join("bucket");
        write(&dir.join("firmware/v1/app.bin"), "firmware");
        write(&dir.join("removed.jsonl"), "");
        (fixture, Remote::Bucket(Bucket::local(&dir)))
    }

    fn apply(fixture: &Fixture, remote: &Remote, products: &[&dyn Product]) -> Result<Applied, Error> {
        let (root, http) = (fixture.root(), Http::new());
        apply_live(&root, &fixture.store, &http, remote, products, None, |_| Ok(()), NO_WAIT)
    }

    fn commit(fixture: &Fixture) {
        let git = ["-c", "user.name=test", "-c", "user.email=test@example.org", "-c", "commit.gpgsign=false"];
        for args in [&["add", "."][..], &["commit", "-q", "-m", "fixture"]] {
            assert!(Command::new("git").args(git).args(args).current_dir(fixture.root()).status().unwrap().success());
        }
    }

    /// Every file in the local bucket.
    fn files(fixture: &Fixture) -> Vec<PathBuf> {
        fn walk(dir: &Path, files: &mut Vec<PathBuf>) {
            for path in std::fs::read_dir(dir).unwrap().map(|entry| entry.unwrap().path()) {
                if path.is_dir() {
                    walk(&path, files);
                } else {
                    files.push(path);
                }
            }
        }
        let mut files = Vec::new();
        walk(&fixture.scratch.0.join("bucket"), &mut files);
        files
    }

    /// Every key in the local bucket, with its bytes.
    fn keys(fixture: &Fixture) -> BTreeMap<String, Vec<u8>> {
        let dir = fixture.scratch.0.join("bucket");
        let key = |path: &Path| path.strip_prefix(&dir).unwrap().to_string_lossy().replace('\\', "/");
        files(fixture).iter().map(|path| (key(path), std::fs::read(path).unwrap())).collect()
    }

    /// Make every key an hour old: an apply removes only keys from before it started.
    fn age(fixture: &Fixture) {
        let hour_ago = SystemTime::now() - Duration::from_secs(3600);
        for path in files(fixture) {
            std::fs::File::options().write(true).open(path).unwrap().set_modified(hour_ago).unwrap();
        }
    }

    /// The id of the live release, once R2 holds exactly what live uses.
    fn checked(fixture: &Fixture, remote: &Remote) -> Option<String> {
        let sources = parse_sources(SOURCES).unwrap();
        let live = Live::read(remote, &[&Versioned], &sources, &fixture.store).unwrap();
        let listed: Vec<Object> = live.swept().iter().flat_map(|prefix| remote.list(prefix).unwrap()).collect();
        let check = live.check(&listed);
        assert!(check.drift.is_empty() && check.leftovers.is_empty(), "R2 holds what live uses: {check:?}");
        live.products[0].release.as_ref().map(|(id, _)| id.clone())
    }

    #[test]
    fn a_first_apply_makes_live_and_removes_what_no_release_uses() {
        let (fixture, remote) = repository("apply-first");
        let dir = fixture.scratch.0.join("bucket");
        write(&dir.join("test/catalog.json"), "{\"schema_version\": 3}");
        write(&dir.join("test/cells/old.obcm"), "old");
        write(&dir.join("reference/v1/16/1.tif"), "reference");
        write(&dir.join("inputs/objects/stray"), "stray");
        age(&fixture);
        write(&dir.join("inputs/objects/fresh"), "another apply uploads it now");

        let applied = apply(&fixture, &remote, &[&Versioned]).unwrap();
        let mut removed: Vec<&str> = applied.removed.iter().map(|object| object.key.as_str()).collect();
        removed.sort();
        assert_eq!(removed, ["inputs/objects/stray", "test/cells/old.obcm"]);
        let keys = keys(&fixture);
        assert!(keys.contains_key("inputs/objects/fresh"), "a key newer than the apply stays");
        assert!(keys.contains_key("reference/v1/16/1.tif"), "no live release reads a `dtm-*` source");
        std::fs::remove_file(dir.join("inputs/objects/fresh")).unwrap();
        let id = checked(&fixture, &remote).unwrap();
        assert_eq!(applied.switched.iter().map(|s| &s.id).collect::<Vec<_>>(), [&id]);
        let mut pointer: serde_json::Value = serde_json::from_slice(&keys["test/catalog.json"]).unwrap();
        let applied = pointer.as_object_mut().unwrap().remove("applied").unwrap().as_str().unwrap().to_string();
        assert!(date::seconds(&applied).unwrap().abs_diff(date::now()) < 600, "the time of the switch: {applied}");
        assert_eq!(pointer, serde_json::json!({"schema": 1, "release": id}));
        let live = Live::read(&remote, &[&Versioned], &[], &fixture.store).unwrap();
        assert_eq!(live.products[0].applied.as_ref(), Some(&applied), "status reads it");
        assert_eq!(
            keys[&format!("test/releases/{id}/COUNT.txt")],
            std::fs::read(fixture.store.object(&live.products[0].release.as_ref().unwrap().1.named[0].sha256)).unwrap()
        );
        assert_eq!(keys[&format!("inputs/objects/{}", sha256_hex(b"head\n"))], b"head\n", "the input copy");
        assert_eq!(keys["firmware/v1/app.bin"], b"firmware", "an apply never touches another prefix");
        let log = String::from_utf8(keys["removed.jsonl"].clone()).unwrap();
        assert_eq!(log.lines().count(), 2, "{log}");

        assert!(apply(&fixture, &remote, &[&Versioned]).unwrap().built.is_none(), "live has every change");

        // R2 lost the record of an input copy: the store gives it, and its objects stay.
        let record = keys.keys().find(|key| key.starts_with("inputs/records/head/2020-01-01/")).unwrap().clone();
        std::fs::remove_file(dir.join(&record)).unwrap();
        age(&fixture);
        let repaired = apply(&fixture, &remote, &[&Versioned]).unwrap();
        assert_eq!(repaired.uploaded, [record]);
        assert!(repaired.switched.is_empty() && repaired.removed.is_empty(), "{repaired:?}");
        checked(&fixture, &remote);
    }

    #[test]
    fn a_key_with_another_size_is_removed_and_uploaded_again() {
        let (fixture, remote) = repository("apply-size");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        let object = files(&fixture).into_iter().find(|path| path.to_string_lossy().contains("test/objects/")).unwrap();
        let bytes = std::fs::read(&object).unwrap();
        std::fs::write(&object, "torn").unwrap();
        age(&fixture);

        let repaired = apply(&fixture, &remote, &[&Versioned]).unwrap();
        let key = object.strip_prefix(fixture.scratch.0.join("bucket")).unwrap().to_string_lossy().replace('\\', "/");
        assert_eq!(repaired.uploaded, [key]);
        assert_eq!(std::fs::read(&object).unwrap(), bytes);
        assert!(String::from_utf8(keys(&fixture)["removed.jsonl"].clone()).unwrap().contains("another size"));
        checked(&fixture, &remote);
    }

    #[test]
    fn named_metadata_repairs_without_builds_and_extra_active_release_files_are_removed() {
        let (fixture, remote) = repository("apply-named");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        let dir = fixture.scratch.0.join("bucket");
        let id = checked(&fixture, &remote).unwrap();
        let key = format!("test/releases/{id}/COUNT.txt");
        let bytes = std::fs::read(dir.join(&key)).unwrap();
        std::fs::remove_file(dir.join(&key)).unwrap();
        let extra = format!("test/releases/{id}/extra.json");
        write(&dir.join(&extra), "{}");
        age(&fixture);
        let repaired = apply(&fixture, &remote, &[&Versioned]).unwrap();
        assert!(repaired.built.as_ref().unwrap().layers.is_empty(), "local receipt bytes repair without a build");
        let run = &repaired.run;
        assert_eq!(&repaired.built.as_ref().unwrap().run, run);
        let events = crate::engine::runs::events(&fixture.store, run).unwrap();
        assert_eq!(events.iter().filter(|event| matches!(event, Event::Started { .. })).count(), 1);
        assert_eq!(events.iter().filter(|event| matches!(event, Event::Finished { .. })).count(), 1);
        assert!(events
            .iter()
            .any(|event| matches!(event, Event::Published { mutation: Publication::Uploaded { .. } })));
        assert_eq!(repaired.uploaded, std::slice::from_ref(&key));
        assert!(repaired.switched.is_empty());
        assert_eq!(repaired.removed.iter().map(|object| object.key.as_str()).collect::<Vec<_>>(), [extra]);
        assert_eq!(std::fs::read(dir.join(&key)).unwrap(), bytes);
        checked(&fixture, &remote);

        let live = Live::read(&remote, &[&Versioned], &[], &fixture.store).unwrap();
        let named = &live.products[0].release.as_ref().unwrap().1.named[0];
        let local = fixture.store.object(&named.sha256);
        std::fs::remove_file(&local).unwrap();
        live.restore_named(&remote, &fixture.store).unwrap();
        assert_eq!(std::fs::read(&local).unwrap(), bytes, "published metadata restores exact local bytes");

        std::fs::remove_file(&local).unwrap();
        std::fs::remove_file(dir.join(&key)).unwrap();
        let rebuilt = apply(&fixture, &remote, &[&Versioned]).unwrap();
        assert_eq!(
            rebuilt.built.as_ref().unwrap().layers.iter().map(|layer| layer.step.as_str()).collect::<Vec<_>>(),
            ["test/count"],
            "only the missing named file's producer builds"
        );
        assert_eq!(rebuilt.uploaded, std::slice::from_ref(&key));
        assert!(rebuilt.switched.is_empty());

        std::fs::write(dir.join(&key), vec![b'x'; bytes.len()]).unwrap();
        let before = keys(&fixture);
        let err = apply(&fixture, &remote, &[&Root]).unwrap_err();
        assert_eq!(err.code, Code::VerifyFailed, "existing immutable named bytes cannot be silently replaced");
        assert_eq!(keys(&fixture), before);

        std::fs::remove_file(&local).unwrap();
        let before = keys(&fixture);
        let err = apply(&fixture, &remote, &[&Versioned]).unwrap_err();
        assert_eq!(err.code, Code::VerifyFailed);
        assert_eq!(keys(&fixture), before, "a bad immutable metadata digest does not mutate R2");
        assert!(!local.exists());
    }

    struct Named(&'static str);

    impl Product for Named {
        fn name(&self) -> &'static str {
            "test"
        }
        fn steps(
            &self,
            root: &std::path::Path,
            env: &Env,
            regions: &Regions,
            store: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Versioned.steps(root, env, regions, store)
        }
        fn named(&self, release: &crate::engine::release::Release) -> Result<Vec<crate::engine::LayerFile>, String> {
            let mut files = Versioned.named(release)?;
            files[0].path = self.0.into();
            Ok(files)
        }
        fn pointer(&self) -> Option<PointerFn> {
            Versioned.pointer()
        }
    }

    #[test]
    fn a_named_path_change_plans_publication_without_rebuilding_the_receipt() {
        let (fixture, remote) = repository("apply-named-path");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        let previous = Live::read(&remote, &[&Versioned], &[], &fixture.store).unwrap();
        let old = previous.products[0].release.as_ref().unwrap();
        let plan = build_cli::plan_live(
            &fixture.root(),
            &fixture.store,
            &Http::new(),
            &remote,
            &[&Named("TOTAL.txt")],
            &[],
            false,
        )
        .unwrap();
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].id, "pointer:test");
        assert!(plan.groups[0].builds.is_empty() && plan.groups[0].layers.is_empty());
        let Some(crate::engine::plan::Cause::Pointer { release, .. }) = &plan.groups[0].cause else {
            panic!("no publication cause")
        };
        assert_ne!(release, &old.0);
        let other = build_cli::plan_live(
            &fixture.root(),
            &fixture.store,
            &Http::new(),
            &remote,
            &[&Named("OTHER.txt")],
            &[],
            false,
        )
        .unwrap();
        assert!(
            !crate::engine::plan::Plan { groups: plan.groups.clone() }
                .same_work(&crate::engine::plan::Plan { groups: other.groups }),
            "a saved plan fixes the desired named identity too"
        );
        age(&fixture);
        let applied = apply(&fixture, &remote, &[&Named("TOTAL.txt")]).unwrap();
        assert!(applied.built.as_ref().unwrap().layers.is_empty());
        assert_eq!(&applied.switched[0].id, release);
        let live = Live::read(&remote, &[&Named("TOTAL.txt")], &[], &fixture.store).unwrap();
        assert_eq!(live.products[0].release.as_ref().unwrap().1.layers, old.1.layers);
        assert_eq!(live.products[0].document, previous.products[0].document, "the inline root body is unchanged");
        assert!(keys(&fixture).contains_key(&format!("test/releases/{release}/TOTAL.txt")));
        assert!(!keys(&fixture).contains_key(&format!("test/releases/{}/COUNT.txt", old.0)));
    }

    struct Root;

    impl Product for Root {
        fn name(&self) -> &'static str {
            "test"
        }
        fn steps(
            &self,
            root: &std::path::Path,
            env: &Env,
            regions: &Regions,
            store: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Versioned.steps(root, env, regions, store)
        }
        fn named(&self, release: &crate::engine::release::Release) -> Result<Vec<crate::engine::LayerFile>, String> {
            Versioned.named(release)
        }
        fn pointer(&self) -> Option<PointerFn> {
            Some(|_, release, _| {
                Ok(Pointer {
                    document: [("schema".into(), 2.into()), ("bound".into(), release.id().into())]
                        .into_iter()
                        .collect(),
                })
            })
        }
    }

    #[test]
    fn a_pointer_only_change_is_planned_and_verified_without_a_layer_build() {
        let (fixture, remote) = repository("apply-pointer");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        let id = checked(&fixture, &remote).unwrap();
        let plan =
            build_cli::plan_live(&fixture.root(), &fixture.store, &Http::new(), &remote, &[&Root], &[], false).unwrap();
        assert_eq!(plan.groups.len(), 1);
        assert_eq!(plan.groups[0].id, "pointer:test");
        assert!(plan.groups[0].builds.is_empty());
        assert!(matches!(plan.groups[0].cause, Some(crate::engine::plan::Cause::Pointer { .. })));
        let applied = apply(&fixture, &remote, &[&Root]).unwrap();
        assert!(applied.built.as_ref().unwrap().layers.is_empty());
        assert!(applied.uploaded.is_empty() && applied.removed.is_empty());
        assert_eq!(applied.switched[0].id, id);
        let document: serde_json::Value = serde_json::from_slice(&keys(&fixture)["test/catalog.json"]).unwrap();
        assert_eq!((document["schema"].as_u64(), document["bound"].as_str()), (Some(2), Some(id.as_str())));
        assert!(apply(&fixture, &remote, &[&Root]).unwrap().built.is_none());

        let dir = fixture.scratch.0.join("bucket");
        write(&dir.join("test/catalog.json"), &format!("{{\"schema\": 99, \"release\": \"{id}\"}}"));
        let before = keys(&fixture);
        let err = apply(&fixture, &remote, &[&Failing]).unwrap_err();
        assert_eq!(err.code, Code::VerifyFailed);
        assert_eq!(keys(&fixture), before, "pointer-only changes also pass verification before mutation");
    }

    #[test]
    fn an_apply_that_moves_a_source_removes_unread_input_files_and_the_old_record() {
        let (fixture, remote) = repository("apply-move");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        let old = checked(&fixture, &remote).unwrap();

        upstream(&fixture, "head", "2020-02-01");
        fixture.fetched_version("head", "2020-02-01", "head.txt", b"newer\n");
        fixture.fetched_version("head", "2020-02-01", "kept.txt", b"head\n");
        age(&fixture);
        let applied = apply(&fixture, &remote, &[&Versioned]).unwrap();
        assert_ne!(checked(&fixture, &remote).unwrap(), old);
        let removed: Vec<&str> = applied.removed.iter().map(|object| object.key.as_str()).collect();
        let old_record = files(&fixture)
            .into_iter()
            .find(|path| path.to_string_lossy().contains("inputs/records/head/2020-01-01/"))
            .map(|path| path.strip_prefix(fixture.scratch.0.join("bucket")).unwrap().to_string_lossy().into_owned());
        let key = format!("test/releases/{old}.json");
        assert!(removed.contains(&key.as_str()), "{key} in {removed:?}");
        let keys = keys(&fixture);
        assert!(old_record.is_none(), "the old read record is removed");
        assert!(keys.keys().any(|key| key.starts_with("inputs/records/head/2020-02-01/")));
        assert!(
            !keys.contains_key(&format!("inputs/objects/{}", sha256_hex(b"head\n"))),
            "an unread file of the new snapshot is not copied"
        );
        assert!(!keys.contains_key(&format!("test/releases/{old}/COUNT.txt")));
    }

    /// The test product, whose release fails its check.
    struct Failing;

    impl Product for Failing {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(
            &self,
            root: &std::path::Path,
            env: &Env,
            regions: &Regions,
            store: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Versioned.steps(root, env, regions, store)
        }

        fn pointer(&self) -> Option<PointerFn> {
            Versioned.pointer()
        }

        fn named(&self, release: &crate::engine::release::Release) -> Result<Vec<crate::engine::LayerFile>, String> {
            Versioned.named(release)
        }

        fn verify(
            &self,
            _: &Path,
            _: Option<&crate::engine::release::Release>,
            _: &crate::engine::release::Release,
            _: &Store,
        ) -> Result<(), String> {
            Err("the reader cannot open it".into())
        }
    }

    #[test]
    fn a_failed_verify_switches_nothing_and_removes_nothing() {
        let (fixture, remote) = repository("apply-verify");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        write(&fixture.scratch.0.join("bucket/test/objects/old"), "old");
        age(&fixture);
        write(&fixture.root().join("join.py"), &JOIN.replace("upper + tail", "tail + upper"));
        commit(&fixture);
        let before = keys(&fixture);
        let err = apply(&fixture, &remote, &[&Failing]).unwrap_err();
        let events = crate::engine::runs::events(&fixture.store, err.run.as_ref().unwrap()).unwrap();
        assert!(matches!(events.last(), Some(Event::Finished { ok: false, .. })));
        assert!(events.iter().any(|event| matches!(event, Event::Phase { phase: Phase::Verify })));
        assert!(!events.iter().any(|event| matches!(event, Event::Published { .. })));
        assert_eq!((err.code, err.code.exit()), (Code::VerifyFailed, 5), "{}", err.message);
        assert_eq!(keys(&fixture), before);
    }

    #[test]
    fn a_no_change_apply_still_needs_consent_and_checks_the_complete_product() {
        let (fixture, remote) = repository("approval-no-change-verify");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        let root = fixture.root();
        let plan = build_cli::plan_live(&root, &fixture.store, &Http::new(), &remote, &[&Failing], &[], false).unwrap();
        assert!(plan.groups.is_empty() && plan.remove.is_empty());
        let original = keys(&fixture);
        let approval = crate::approval::read(&fixture.store).unwrap();
        let asked = std::cell::Cell::new(false);
        let error = apply_live(
            &root,
            &fixture.store,
            &Http::new(),
            &remote,
            &[&Failing],
            Some(&plan),
            |reviewed| {
                assert_eq!(reviewed, &plan);
                asked.set(true);
                Ok(())
            },
            NO_WAIT,
        )
        .unwrap_err();
        assert!(asked.get(), "no-op publication cannot silently establish approval");
        assert_eq!(error.code, Code::VerifyFailed);
        assert_eq!(keys(&fixture), original);
        assert_eq!(crate::approval::read(&fixture.store).unwrap(), approval);
        assert!(!crate::engine::runs::events(&fixture.store, error.run.as_ref().unwrap())
            .unwrap()
            .iter()
            .any(|event| matches!(event, Event::Published { .. })));
    }

    #[test]
    fn a_cleanup_failure_keeps_the_run_and_its_acknowledged_pointer_switch() {
        let (fixture, remote) = repository("apply-cleanup-journal");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        let old = checked(&fixture, &remote).unwrap();
        age(&fixture);
        write(&fixture.root().join("join.py"), &JOIN.replace("upper + tail", "tail + upper"));
        commit(&fixture);
        let root = fixture.root();
        let log = fixture.scratch.0.join("bucket/removed.jsonl");
        let error = apply_live(
            &root,
            &fixture.store,
            &Http::new(),
            &remote,
            &[&Versioned],
            None,
            |_| {
                std::fs::remove_file(&log).unwrap();
                std::fs::create_dir(&log).unwrap();
                Ok(())
            },
            NO_WAIT,
        )
        .unwrap_err();
        assert_eq!(error.code, Code::R2Failed);
        let id = error.run.as_ref().unwrap();
        let events = crate::engine::runs::events(&fixture.store, id).unwrap();
        assert!(matches!(events.last(), Some(Event::Finished { ok: false, .. })));
        let switched = events
            .iter()
            .find_map(|event| match event {
                Event::Published { mutation: Publication::Switched { release, .. } } => Some(release),
                _ => None,
            })
            .expect("the pointer put acknowledged success before cleanup failed");
        assert_ne!(switched, &old);
        let details = crate::engine::runs::details(&fixture.store, id).unwrap();
        assert_eq!(details.phase, Some(Phase::Cleanup));
        assert!(details.published.iter().any(|write| matches!(write, Publication::Switched { .. })));
        assert_eq!(events.iter().filter(|event| matches!(event, Event::Finished { .. })).count(), 1);
    }

    struct Selected(crate::engine::Client);

    impl Product for Selected {
        fn name(&self) -> &'static str {
            "test"
        }
        fn pointer(&self) -> Option<PointerFn> {
            Versioned.pointer()
        }

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Ok(vec![crate::engine::tests::packaged(self.0.clone())].into())
        }
    }

    #[test]
    fn client_selection_changes_publication_and_cleanup_without_rebuilding_bytes() {
        use crate::engine::Client;
        let (fixture, remote) = repository("apply-selection");
        apply(&fixture, &remote, &[&Selected(Client::All)]).unwrap();
        age(&fixture);
        let product = Selected(Client::Paths(vec!["published".into()]));
        let applied = apply(&fixture, &remote, &[&product]).unwrap();
        assert!(applied.built.as_ref().unwrap().layers.is_empty(), "the bytes are reused");
        for bytes in [b"other".as_slice(), b"metadata"] {
            let key = format!("test/objects/{}", sha256_hex(bytes));
            assert!(applied.removed.iter().any(|object| object.key == key));
            assert!(!keys(&fixture).contains_key(&key));
        }
        let selected = format!("test/objects/{}", sha256_hex(b"payload"));
        assert!(keys(&fixture).contains_key(&selected));
        let live = Live::read(&remote, &[&product], &[], &fixture.store).unwrap();
        assert!(live.owners(&[selected]).contains("test/package"));
        let private = format!("test/objects/{}", sha256_hex(b"metadata"));
        assert!(live.owners(std::slice::from_ref(&private)).is_empty());
        age(&fixture);
        let expanded = apply(&fixture, &remote, &[&Selected(Client::All)]).unwrap();
        assert!(expanded.built.as_ref().unwrap().layers.is_empty());
        assert!(expanded.uploaded.contains(&private), "the store retains private bytes");
    }

    #[test]
    fn blocked_required_layers_refuse_apply_and_preserve_the_complete_live_release() {
        let (fixture, remote) = repository("apply-partial");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        age(&fixture);
        write(&fixture.root().join("join.py"), &JOIN.replace("upper + tail", "tail + upper"));
        commit(&fixture);
        let before = keys(&fixture);
        let error = apply(&fixture, &remote, &[&super::super::build_cli::tests::Partial]).unwrap_err();
        assert_eq!(error.code, Code::Blocked);
        assert!(error.message.contains("test/missing"));
        assert_eq!(keys(&fixture), before);
        let args = BuildArgs { env: "live".into(), only: Vec::new(), plan: None, moves: Vec::new() };
        let mut run = Run::create(&fixture.store, "partial build").unwrap();
        let (built, applying) = build_cli::build_env(
            &fixture.root(),
            &fixture.store,
            &Http::new(),
            Some(&remote),
            &[&super::super::build_cli::tests::Partial],
            &args,
            None,
            &mut run,
        )
        .unwrap();
        assert!(built.releases.is_empty());
        let applying = applying.unwrap();
        assert_eq!(applying.next.products[0].release, applying.live.products[0].release);
        assert_eq!(keys(&fixture), before);
    }

    /// A product without a pointer, whose older publish R2 holds.
    struct Other;

    impl Product for Other {
        fn name(&self) -> &'static str {
            "other"
        }

        fn steps(
            &self,
            _root: &std::path::Path,
            _: &Env,
            _: &Regions,
            _: &Store,
        ) -> Result<crate::product::Steps, Unplanned> {
            Ok(Vec::new().into())
        }
    }

    #[test]
    fn a_product_without_a_pointer_is_left_out_and_the_others_apply() {
        let (fixture, remote) = repository("apply-blocked");
        let dir = fixture.scratch.0.join("bucket");
        write(&dir.join("other/catalog.json"), "{\"schema_version\": 3}");
        write(&dir.join("other/cells/a.obcm"), "cell");
        age(&fixture);

        let applied = apply(&fixture, &remote, &[&Versioned, &Other]).unwrap();
        let blocked = &applied.built.as_ref().unwrap().blocked;
        assert_eq!(blocked.iter().map(|b| b.product.as_str()).collect::<Vec<_>>(), ["other"]);
        assert_eq!(applied.switched.iter().map(|s| s.product.as_str()).collect::<Vec<_>>(), ["test"]);
        let keys = keys(&fixture);
        assert_eq!(keys["other/cells/a.obcm"], b"cell", "a product with nothing live owns no prefix");

        let err = apply(&fixture, &remote, &[&Other]).unwrap_err();
        assert_eq!((err.code, err.code.exit()), (Code::Blocked, 4), "no product applies: {}", err.message);
    }

    #[test]
    fn an_apply_that_stops_before_the_switch_leaves_live_and_its_plan_applies_again() {
        let (fixture, remote) = repository("apply-retry");
        let (root, http, products) = (fixture.root(), Http::new(), [&Versioned as &dyn Product]);
        let Remote::Bucket(bucket) = &remote else { unreachable!() };
        let plan = build_cli::plan_live(&root, &fixture.store, &http, &remote, &products, &[], true).unwrap();
        let scratch = Scratch::new().unwrap();
        let mut run = Run::create(&fixture.store, "stage only").unwrap();
        let (_, next) = stage(&root, &fixture.store, &http, &remote, &products, &plan, &mut run).unwrap();
        let expected = super::super::commit_cli::expected(&plan, &products).unwrap();
        let directory = scratch.0.join("prepared");
        let digest = super::super::commit_cli::pack(
            &directory,
            &fixture.store,
            &run,
            expected,
            next,
            parse_sources(SOURCES).unwrap(),
            &remote,
            (Vec::new(), Vec::new()),
            plan.approval.clone().unwrap(),
        )
        .unwrap();
        let payload = Store::at(&directory);
        let (_, next) = stage(&root, &fixture.store, &http, &remote, &products, &plan, &mut run).unwrap();
        for (_, _, release) in next.releases() {
            release.write(&payload).unwrap();
        }
        let files = super::files(&payload, &scratch, &next).unwrap();
        let mut owner =
            crate::commit::Owner::open(&fixture.store.root().join("commits"), run.id(), digest.as_bytes()).unwrap();
        let staged = super::upload(bucket, &next.list(&remote).unwrap(), &files, &mut run, &mut owner).unwrap();
        drop(owner);
        drop(run);
        // The process stops here.
        let sources = parse_sources(SOURCES).unwrap();
        let live = Live::read(&remote, &products, &sources, &fixture.store).unwrap();
        assert!(live.products[0].release.is_none(), "live does not change before the switch");
        let copy = format!("inputs/objects/{}", sha256_hex(b"head\n"));
        assert!(staged.contains(&copy), "{staged:?}");

        let retry = apply_live(&root, &fixture.store, &http, &remote, &products, Some(&plan), |_| Ok(()), NO_WAIT);
        let retry = retry.unwrap();
        assert!(retry.uploaded.is_empty(), "nothing uploads twice: {:?}", retry.uploaded);
        assert_eq!((retry.switched.len(), retry.removed.len()), (1, 0));
        checked(&fixture, &remote).unwrap();
        assert!(keys(&fixture).contains_key(&copy), "the input copy of the stopped apply stays");
    }

    #[test]
    fn a_terminal_asks_unless_yes_and_without_one_an_apply_needs_yes_or_a_plan() {
        let args = |yes, plan: Option<&str>| ApplyArgs { env: "live".into(), plan: plan.map(PathBuf::from), yes };
        let err = consent(&args(false, None), false).unwrap_err();
        assert_eq!((err.code, err.code.exit()), (Code::NoTerminal, 2));
        assert!(consent(&args(true, None), false).unwrap());
        assert!(consent(&args(false, Some("plan.json")), false).unwrap());
        assert!(!consent(&args(false, None), true).unwrap(), "a terminal asks");
        assert!(!consent(&args(false, Some("plan.json")), true).unwrap(), "a terminal asks for a plan too");
        assert!(consent(&args(true, Some("plan.json")), true).unwrap());
    }

    #[test]
    fn consent_keeps_exact_pointer_bytes_even_when_the_document_is_equivalent() {
        let (fixture, remote) = repository("apply-exact-consent");
        apply(&fixture, &remote, &[&Versioned]).unwrap();
        let original = checked(&fixture, &remote).unwrap();
        age(&fixture);
        write(&fixture.root().join("join.py"), &JOIN.replace("upper + tail", "tail + upper"));
        commit(&fixture);
        let pointer = fixture.scratch.0.join("bucket/test/catalog.json");
        let result = apply_live(
            &fixture.root(),
            &fixture.store,
            &Http::new(),
            &remote,
            &[&Versioned],
            None,
            |_| {
                let body = std::fs::read(&pointer).unwrap();
                let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
                std::fs::write(&pointer, serde_json::to_vec(&value).unwrap()).unwrap();
                Ok(())
            },
            NO_WAIT,
        );
        assert_eq!(result.unwrap_err().code, Code::PlanOutdated);
        let value: serde_json::Value = serde_json::from_slice(&std::fs::read(pointer).unwrap()).unwrap();
        assert_eq!(value["release"], original);
    }

    #[test]
    fn the_owner_rechecks_absence_after_preparation_before_any_upload() {
        let (fixture, remote) = repository("commit-stale-absence");
        let root = fixture.root();
        let products = [&Versioned as &dyn Product];
        let plan = build_cli::plan_live(&root, &fixture.store, &Http::new(), &remote, &products, &[], true).unwrap();
        let mut run = Run::create(&fixture.store, "prepare commit").unwrap();
        let (_, next) = stage(&root, &fixture.store, &Http::new(), &remote, &products, &plan, &mut run).unwrap();
        let scratch = Scratch::new().unwrap();
        let directory = scratch.0.join("bundle");
        let digest = super::super::commit_cli::pack(
            &directory,
            &fixture.store,
            &run,
            super::super::commit_cli::expected(&plan, &products).unwrap(),
            next,
            parse_sources(SOURCES).unwrap(),
            &remote,
            (Vec::new(), Vec::new()),
            plan.approval.clone().unwrap(),
        )
        .unwrap();
        drop(run);
        write(&fixture.scratch.0.join("bucket/test/catalog.json"), "{\"schema\":99}");
        let before = keys(&fixture);
        let error = super::super::commit_cli::execute_for_test(&directory, &digest, &fixture.store, &remote, NO_WAIT)
            .unwrap_err();
        assert_eq!(error.code, Code::PlanOutdated);
        assert_eq!(keys(&fixture), before);
    }

    #[test]
    fn an_apply_refuses_data_that_git_does_not_have() {
        let (fixture, remote) = repository("apply-uncommitted");
        let root = fixture.root();
        write(&root.join("data/env/local.toml"), "region = \"monaco\"\n");
        assert!(committed(&root).is_ok(), "local.toml is never in git");
        write(&root.join("data/env/live.toml"), "region = \"andorra\"\n");
        let before = keys(&fixture);
        let err = apply(&fixture, &remote, &[&Versioned]).unwrap_err();
        let message = "data/ has changes that are not committed: data/env/live.toml";
        assert_eq!((err.code, err.message.as_str()), (Code::Usage, message));
        assert_eq!(keys(&fixture), before);
    }
}
