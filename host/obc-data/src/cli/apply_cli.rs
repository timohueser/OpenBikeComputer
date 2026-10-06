//! `obc data apply live`: build the plan of live and check its releases, upload what R2 lacks,
//! switch the pointer of each product that the plan changes, and then remove from R2 what no live
//! release uses. Until the pointers switch, live does not change.

use std::collections::BTreeMap;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Args;
use schemars::JsonSchema;
use serde::Serialize;

use super::build_cli::{self, Applying, BuildArgs, Built, BuiltRelease, EnvPlan};
use super::{bytes, confirm, registry, Code, Error};
use crate::fetch::http::Http;
use crate::live::{Live, Remote, INPUTS};
use crate::product::{Pointer, Product};
use crate::r2::{Bucket, Object, Upload};
use crate::sources::Source;
use crate::store::{hash_file, write_atomic, Store};

/// How long the files of the releases before a switch stay on R2: a client that read an old
/// pointer finishes its downloads.
const WAIT: Duration = Duration::from_secs(600);

/// A pointer is at most this old in a cache, so a switch reaches every client soon.
const POINTER_CACHE: &str = "public, max-age=60, must-revalidate";
const JSON: &str = "application/json";
const IMMUTABLE: Upload<'static> =
    Upload { cache_control: Some("public, max-age=31536000, immutable"), content_type: None, immutable: true };

#[derive(Args)]
pub struct ApplyArgs {
    /// The environment. Only `live` applies.
    env: String,
    /// Apply this output of `plan live --json`. Exit status 3 when live or the plan of now differs.
    #[arg(long)]
    plan: Option<PathBuf>,
    /// Do not ask. Without a terminal, this or `--plan` is required.
    #[arg(long)]
    yes: bool,
}

/// What an apply did.
#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct Applied {
    /// The build of the plan; `null` when live had every change.
    pub built: Option<Built>,
    /// The keys that it uploaded.
    pub uploaded: Vec<String>,
    /// The release of each product whose pointer it switched.
    pub switched: Vec<BuiltRelease>,
    /// The keys that it removed: no live release used them.
    pub removed: Vec<Object>,
}

pub fn apply(root: &Path, products: &[&dyn Product], args: ApplyArgs, json: bool) -> Result<(), Error> {
    if args.env != "live" {
        return Err(Code::Usage.error(format!("`apply {}`: only `live` applies", args.env)));
    }
    let consent = consent(&args, std::io::stdin().is_terminal())?;
    let saved = args.plan.as_deref().map(build_cli::read_plan).transpose()?;
    let saved = args.plan.as_deref().zip(saved.as_ref());
    let ask = |plan: &EnvPlan| {
        if !json {
            build_cli::print_plan(plan);
        }
        confirm(&question(plan), consent)
    };
    let (store, remote) = (Store::open()?, super::remote()?);
    let applied = apply_live(root, &store, &Http::new(), &remote, products, saved, ask, WAIT)?;
    if json {
        return super::print_json(&applied);
    }
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

/// Whether the apply has consent without a question: `--yes` or `--plan`. Without them, it needs a
/// terminal to ask in.
fn consent(args: &ApplyArgs, terminal: bool) -> Result<bool, Error> {
    let consent = args.yes || args.plan.is_some();
    if !consent && !terminal {
        return Err(Code::NoTerminal.error("there is no terminal to ask in; nothing changed"));
    }
    Ok(consent)
}

/// The one question before an apply.
fn question(plan: &EnvPlan) -> String {
    let removes = bytes(plan.remove.iter().filter_map(|removal| removal.bytes).sum());
    let changes = match plan.groups.len() {
        1 => "1 change".into(),
        n => format!("{n} changes"),
    };
    format!("Apply {changes} to live? removes {removes} from R2")
}

/// Apply `saved`, the plan in the file that it names, or else the plan of now. `ask` gets the
/// plan before anything changes; `wait` passes between the switch and the removal.
#[allow(clippy::too_many_arguments)]
fn apply_live(
    root: &Path,
    store: &Store,
    http: &Http,
    remote: &Remote,
    products: &[&dyn Product],
    saved: Option<(&Path, &EnvPlan)>,
    ask: impl FnOnce(&EnvPlan) -> Result<(), Error>,
    wait: Duration,
) -> Result<Applied, Error> {
    let Remote::Bucket(bucket) = remote else {
        return Err(Code::Blocked.error("an apply writes R2: the OBC_R2_* variables are not set"));
    };
    committed(root)?;
    let _lock = store.try_lock("apply-live")?.ok_or_else(|| {
        Code::Usage.error("another apply of live runs on this machine").fix("Wait for it to end, then plan again.")
    })?;
    let now;
    let saved = match saved {
        Some(saved) => saved,
        None => {
            now = build_cli::plan_live(root, store, http, remote, products)?;
            (Path::new("the plan that you confirmed"), &now)
        }
    };
    build_cli::suits(products, saved.1)?;
    pointers(products, saved.1)?;
    if saved.1.groups.is_empty() && saved.1.remove.is_empty() {
        return Ok(Applied::default());
    }
    ask(saved.1)?;

    let scratch = Scratch::new(store);
    let (built, switches, uploaded) = stage(root, http, (remote, bucket), products, saved, &scratch)?;
    let mut switched = Vec::new();
    for switch in switches {
        let key = format!("{}/catalog.json", switch.prefix);
        let mut document = switch.pointer.document;
        document.insert("release".into(), switch.id.clone().into());
        let file = scratch.write(&key, &serde_json::to_vec_pretty(&document).map_err(|e| e.to_string())?)?;
        let upload = Upload { cache_control: Some(POINTER_CACHE), content_type: Some(JSON), immutable: false };
        bucket.put(&file, &key, &upload).map_err(r2_failed)?;
        bucket.verify(&file, &key).map_err(|e| Code::VerifyFailed.error(e))?;
        switched.push(BuiltRelease { product: switch.product.into(), id: switch.id });
    }

    let sources = registry(root)?.sources;
    let mut removed = leftovers(remote, products, &sources, store)?;
    if !removed.is_empty() && !switched.is_empty() {
        eprintln!("obc data: the old releases stay {} s for clients that read the old pointers", wait.as_secs());
        std::thread::sleep(wait);
        removed = leftovers(remote, products, &sources, store)?;
    }
    if !removed.is_empty() {
        bucket.delete(&removed, "obc data apply live: no live release uses it").map_err(r2_failed)?;
    }
    Ok(Applied { built: Some(built), uploaded, switched, removed })
}

/// Build the plan `saved`, check its releases and upload what R2 lacks. Live does not change.
fn stage<'a>(
    root: &Path,
    http: &Http,
    (remote, bucket): (&Remote, &Bucket),
    products: &[&'a dyn Product],
    saved: (&Path, &EnvPlan),
    scratch: &Scratch,
) -> Result<(Built, Vec<Switch<'a>>, Vec<String>), Error> {
    let (store, args) =
        (scratch.store, BuildArgs { env: "live".into(), only: Vec::new(), plan: None, moves: Vec::new() });
    let (built, applying) = build_cli::build_env(root, store, http, Some(remote), products, &args, Some(saved))?;
    let Applying { live, next } = applying.expect("a build of live gives what an apply changes");
    let switches = switches(products, store, &live, &next)?;
    let files = files(scratch, &next, &switches)?;
    let uploaded = upload(bucket, &next.list(remote).map_err(r2_failed)?, &files)?;
    Ok((built, switches, uploaded))
}

fn r2_failed(message: String) -> Error {
    Code::R2Failed.error(message)
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

/// Refuse a plan that changes the release of a product without a pointer: an apply cannot make
/// that release live.
fn pointers(products: &[&dyn Product], plan: &EnvPlan) -> Result<(), Error> {
    for product in products.iter().filter(|product| product.pointer().is_none()) {
        let name = product.name();
        let mine = |step: &str| step.split('/').next() == Some(name);
        let mut steps =
            plan.groups.iter().flat_map(|group| group.layers.iter().map(|layer| &layer.step).chain(&group.drops));
        if plan.edits.iter().any(|edit| edit.product() == name) || steps.any(|step| mine(step)) {
            let message =
                format!("product `{name}` has no client document yet, so an apply cannot make its release live");
            return Err(Code::Blocked.error(message).fix(format!("Give product `{name}` a pointer in its crate.")));
        }
    }
    Ok(())
}

/// A product whose release an apply changes.
struct Switch<'a> {
    product: &'a str,
    prefix: String,
    id: String,
    pointer: Pointer,
}

/// Each product whose release `next` changes, with its pointer. Each release is checked here,
/// before anything changes on R2.
fn switches<'a>(
    products: &[&'a dyn Product],
    store: &Store,
    live: &Live,
    next: &Live,
) -> Result<Vec<Switch<'a>>, Error> {
    let mut switches = Vec::new();
    for (product, (live, next)) in products.iter().zip(live.products.iter().zip(&next.products)) {
        let Some((id, release)) = next.release.as_ref().filter(|_| build_cli::changed(live, next)) else {
            continue;
        };
        let name = product.name();
        let failed = |e: String| {
            Code::VerifyFailed
                .error(format!("release {} of `{name}`: {e}; nothing changed", &id[..8]))
                .fix(format!("Correct the steps of product `{name}`, then plan again."))
        };
        product.verify(release, store).map_err(failed)?;
        let pointer =
            product.pointer().ok_or_else(|| Code::Blocked.error(format!("product `{name}` has no pointer")))?;
        let pointer = pointer(release, store).map_err(failed)?;
        switches.push(Switch { product: name, prefix: next.prefix.clone(), id: id.clone(), pointer });
    }
    Ok(switches)
}

/// A key that live needs, and the file that holds its bytes.
struct File {
    key: String,
    path: PathBuf,
    /// The size of the key on R2; `None` when any object at the key serves.
    size: Option<u64>,
    /// The SHA-256 of the file, when the key names it.
    sha256: Option<String>,
    upload: Upload<'static>,
}

/// Every key of `next` but the pointers: the manifests and objects of its releases, the files that
/// a client finds by name of the releases of `switches`, and the input copies.
fn files(scratch: &Scratch, next: &Live, switches: &[Switch]) -> Result<Vec<File>, Error> {
    let store = scratch.store;
    let mut files = Vec::new();
    for (prefix, id, release) in next.releases() {
        files.push(File {
            key: format!("{prefix}/releases/{id}.json"),
            path: store.release(&release.product, id),
            size: Some(release.canonical().len() as u64),
            sha256: Some(id.into()),
            upload: Upload { content_type: Some(JSON), ..IMMUTABLE },
        });
        files.extend(release.objects().into_iter().map(|(sha256, size)| File {
            key: format!("{prefix}/objects/{sha256}"),
            path: store.object(sha256),
            size: Some(size),
            sha256: Some(sha256.into()),
            upload: IMMUTABLE,
        }));
    }
    for switch in switches {
        for (path, bytes) in &switch.pointer.named {
            let key = format!("{}/releases/{}/{path}", switch.prefix, switch.id);
            let size = Some(bytes.len() as u64);
            files.push(File { path: scratch.write(&key, bytes)?, key, size, sha256: None, upload: IMMUTABLE });
        }
    }
    for ((source, version), record) in &next.inputs {
        let record = record.as_ref().ok_or_else(|| {
            Code::Failed
                .error(format!("neither R2 nor the store has the record of the input copy {source}@{version}"))
                .fix(format!("Fetch it with `obc data fetch {source}@{version}`, or plan with `--move {source}`."))
        })?;
        let key = format!("{INPUTS}/records/{source}/{version}.json");
        let bytes = serde_json::to_vec_pretty(record).map_err(|e| e.to_string())?;
        let upload = Upload { content_type: Some(JSON), ..Upload::default() };
        files.push(File { path: scratch.write(&key, &bytes)?, key, size: None, sha256: None, upload });
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
fn upload(bucket: &Bucket, listed: &[Object], files: &[File]) -> Result<Vec<String>, Error> {
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
    for file in missing.values() {
        let Some(sha256) = &file.sha256 else { continue };
        let (found, size) = hash_file(&file.path).map_err(|e| Code::Failed.error(format!("{}: {e}", file.key)))?;
        if (&found, Some(size)) != (sha256, file.size) {
            let message = format!("{} holds other bytes than {}; nothing changed", file.path.display(), file.key);
            return Err(Code::VerifyFailed.error(message).fix("Remove that file from the store, then plan again."));
        }
    }
    if !wrong.is_empty() {
        bucket.delete(&wrong, "obc data apply live: the key holds another size than live needs").map_err(r2_failed)?;
    }
    for file in missing.values() {
        bucket.put(&file.path, &file.key, &file.upload).map_err(r2_failed)?;
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

/// The keys under the prefixes of live that no live release uses, as R2 has them now. Drift is an
/// error: a listing that lacks a key of live is no ground for a removal.
fn leftovers(
    remote: &Remote,
    products: &[&dyn Product],
    sources: &[Source],
    store: &Store,
) -> Result<Vec<Object>, Error> {
    let live = Live::read(remote, products, sources, store).map_err(r2_failed)?;
    let mut listed = Vec::new();
    for prefix in live.swept() {
        listed.extend(remote.list(&prefix).map_err(r2_failed)?);
    }
    let check = live.check(&listed);
    if let Some(drift) = check.drift.first() {
        let message = format!("R2 lacks `{}` of live, so nothing was removed", drift.key);
        return Err(Code::R2Failed.error(message).fix("Run `obc data apply live` again: it uploads what R2 lacks."));
    }
    Ok(check.leftovers)
}

/// A directory in the store for the files that an apply writes; it goes when it goes out of scope.
struct Scratch<'a> {
    store: &'a Store,
    dir: PathBuf,
}

impl<'a> Scratch<'a> {
    fn new(store: &'a Store) -> Self {
        let dir = store.partial(&format!("apply-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        Self { store, dir }
    }

    /// Write `bytes` for `key`, and give the path.
    fn write(&self, key: &str, bytes: &[u8]) -> Result<PathBuf, Error> {
        let path = self.dir.join(key);
        write_atomic(&path, bytes)?;
        Ok(path)
    }
}

impl Drop for Scratch<'_> {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;
    use crate::cli::build_cli::tests::{upstream, Versioned, SOURCES};
    use crate::engine::tests::{fixture, write, Fixture, JOIN};
    use crate::engine::Step;
    use crate::env::Env;
    use crate::product::{PointerFn, Unplanned};
    use crate::regions::Regions;
    use crate::sources::parse_sources;
    use crate::store::sha256_hex;

    /// A repository whose `data/` git has, `head@2020-01-01` in the store, and a local bucket with a
    /// firmware file and the removal log.
    fn repository(name: &str) -> (Fixture, Remote) {
        let fixture = fixture(name);
        let root = fixture.root();
        write(&root.join("data/sources.toml"), SOURCES);
        write(&root.join("data/regions/monaco.toml"), "name = \"Monaco\"\nkind = \"geofabrik\"\n");
        write(&root.join("data/env/live.toml"), "region = \"monaco\"\n");
        let git = ["-c", "user.name=test", "-c", "user.email=test@example.org", "-c", "commit.gpgsign=false"];
        for args in [&["add", "data"][..], &["commit", "-q", "-m", "data"]] {
            assert!(Command::new("git").args(git).args(args).current_dir(&root).status().unwrap().success());
        }
        fixture.fetched_version("head", "2020-01-01", "head.txt", b"head\n");
        upstream(&fixture, "head", "2020-01-01");
        upstream(&fixture, "tail", "1");
        let dir = fixture.scratch.0.join("bucket");
        write(&dir.join("firmware/v1/app.bin"), "firmware");
        write(&dir.join("removed.jsonl"), "");
        (fixture, Remote::Bucket(Bucket::local(&dir)))
    }

    fn apply(fixture: &Fixture, remote: &Remote, product: &dyn Product) -> Result<Applied, Error> {
        let (root, http) = (fixture.root(), Http::new());
        apply_live(&root, &fixture.store, &http, remote, &[product], None, |_| Ok(()), Duration::ZERO)
    }

    /// Every key in the local bucket, with its bytes.
    fn keys(fixture: &Fixture) -> BTreeMap<String, Vec<u8>> {
        fn walk(dir: &Path, base: &Path, keys: &mut BTreeMap<String, Vec<u8>>) {
            for entry in std::fs::read_dir(dir).unwrap().map(Result::unwrap) {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, base, keys);
                } else {
                    let key = path.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
                    keys.insert(key, std::fs::read(&path).unwrap());
                }
            }
        }
        let (dir, mut keys) = (fixture.scratch.0.join("bucket"), BTreeMap::new());
        walk(&dir, &dir, &mut keys);
        keys
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

        let applied = apply(&fixture, &remote, &Versioned).unwrap();
        let id = checked(&fixture, &remote).unwrap();
        assert_eq!(applied.switched.iter().map(|s| &s.id).collect::<Vec<_>>(), [&id]);
        let mut removed: Vec<&str> = applied.removed.iter().map(|object| object.key.as_str()).collect();
        removed.sort();
        assert_eq!(removed, ["inputs/objects/stray", "reference/v1/16/1.tif", "test/cells/old.obcm"]);
        let keys = keys(&fixture);
        let pointer: serde_json::Value = serde_json::from_slice(&keys["test/catalog.json"]).unwrap();
        assert_eq!(pointer, serde_json::json!({"schema": 1, "release": id}));
        assert_eq!(keys[&format!("test/releases/{id}/LICENSE.txt")], b"CC0-1.0\n");
        assert_eq!(keys[&format!("inputs/objects/{}", sha256_hex(b"head\n"))], b"head\n", "the input copy");
        assert_eq!(keys["firmware/v1/app.bin"], b"firmware", "an apply never touches another prefix");
        let log = String::from_utf8(keys["removed.jsonl"].clone()).unwrap();
        assert_eq!(log.lines().count(), 3, "{log}");

        assert!(apply(&fixture, &remote, &Versioned).unwrap().built.is_none(), "live has every change");

        // R2 lost the record of an input copy: the store gives it, and its objects stay.
        std::fs::remove_file(dir.join("inputs/records/head/2020-01-01.json")).unwrap();
        let repaired = apply(&fixture, &remote, &Versioned).unwrap();
        assert_eq!(repaired.uploaded, ["inputs/records/head/2020-01-01.json"]);
        assert!(repaired.switched.is_empty() && repaired.removed.is_empty(), "{repaired:?}");
        checked(&fixture, &remote);
    }

    #[test]
    fn an_apply_that_moves_a_source_keeps_the_shared_object_and_removes_the_old_record() {
        let (fixture, remote) = repository("apply-move");
        apply(&fixture, &remote, &Versioned).unwrap();
        let old = checked(&fixture, &remote).unwrap();

        upstream(&fixture, "head", "2020-02-01");
        fixture.fetched_version("head", "2020-02-01", "head.txt", b"newer\n");
        fixture.fetched_version("head", "2020-02-01", "kept.txt", b"head\n");
        let applied = apply(&fixture, &remote, &Versioned).unwrap();
        assert_ne!(checked(&fixture, &remote).unwrap(), old);
        let removed: Vec<&str> = applied.removed.iter().map(|object| object.key.as_str()).collect();
        for key in ["inputs/records/head/2020-01-01.json".into(), format!("test/releases/{old}.json")] {
            assert!(removed.contains(&key.as_str()), "{key} in {removed:?}");
        }
        let keys = keys(&fixture);
        assert!(keys.contains_key("inputs/records/head/2020-02-01.json"));
        assert_eq!(keys[&format!("inputs/objects/{}", sha256_hex(b"head\n"))], b"head\n", "both versions have it");
        assert!(!keys.contains_key(&format!("test/releases/{old}/LICENSE.txt")));
    }

    /// The test product, whose release fails its check.
    struct Failing;

    impl Product for Failing {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
            Versioned.steps(env, regions, store)
        }

        fn pointer(&self) -> Option<PointerFn> {
            Versioned.pointer()
        }

        fn verify(&self, _: &crate::engine::release::Release, _: &Store) -> Result<(), String> {
            Err("the reader cannot open it".into())
        }
    }

    #[test]
    fn a_failed_verify_switches_nothing_and_removes_nothing() {
        let (fixture, remote) = repository("apply-verify");
        apply(&fixture, &remote, &Versioned).unwrap();
        write(&fixture.scratch.0.join("bucket/test/objects/old"), "old");
        write(&fixture.root().join("join.py"), &JOIN.replace("upper + tail", "tail + upper"));
        let before = keys(&fixture);
        let err = apply(&fixture, &remote, &Failing).unwrap_err();
        assert_eq!((err.code, err.code.exit()), (Code::VerifyFailed, 5), "{}", err.message);
        assert_eq!(keys(&fixture), before);
    }

    /// The test product without a pointer.
    struct Unpointed;

    impl Product for Unpointed {
        fn name(&self) -> &'static str {
            "test"
        }

        fn steps(&self, env: &Env, regions: &Regions, store: &Store) -> Result<Vec<Step>, Unplanned> {
            Versioned.steps(env, regions, store)
        }
    }

    #[test]
    fn a_product_without_a_pointer_is_blocked() {
        let (fixture, remote) = repository("apply-blocked");
        let before = keys(&fixture);
        let err = apply(&fixture, &remote, &Unpointed).unwrap_err();
        assert_eq!((err.code, err.code.exit()), (Code::Blocked, 4), "{}", err.message);
        assert!(err.message.contains("no client document"), "{}", err.message);
        assert_eq!(keys(&fixture), before);
    }

    #[test]
    fn an_apply_that_stops_before_the_switch_leaves_live_and_its_plan_applies_again() {
        let (fixture, remote) = repository("apply-retry");
        let (root, http, products) = (fixture.root(), Http::new(), [&Versioned as &dyn Product]);
        let Remote::Bucket(bucket) = &remote else { unreachable!() };
        let plan = build_cli::plan_live(&root, &fixture.store, &http, &remote, &products).unwrap();
        let saved = (Path::new("plan.json"), &plan);
        let scratch = Scratch::new(&fixture.store);
        let (_, _, staged) = stage(&root, &http, (&remote, bucket), &products, saved, &scratch).unwrap();
        // The process stops here.
        let sources = parse_sources(SOURCES).unwrap();
        let live = Live::read(&remote, &products, &sources, &fixture.store).unwrap();
        assert!(live.products[0].release.is_none(), "live does not change before the switch");
        let copy = format!("inputs/objects/{}", sha256_hex(b"head\n"));
        assert!(staged.contains(&copy), "{staged:?}");

        let retry =
            apply_live(&root, &fixture.store, &http, &remote, &products, Some(saved), |_| Ok(()), Duration::ZERO);
        let retry = retry.unwrap();
        assert!(retry.uploaded.is_empty(), "nothing uploads twice: {:?}", retry.uploaded);
        assert_eq!((retry.switched.len(), retry.removed.len()), (1, 0));
        checked(&fixture, &remote).unwrap();
        assert!(keys(&fixture).contains_key(&copy), "the input copy of the stopped apply stays");
    }

    #[test]
    fn without_a_terminal_an_apply_needs_yes_or_a_plan() {
        let args = |yes, plan: Option<&str>| ApplyArgs { env: "live".into(), plan: plan.map(PathBuf::from), yes };
        let err = consent(&args(false, None), false).unwrap_err();
        assert_eq!((err.code, err.code.exit()), (Code::NoTerminal, 2));
        assert!(consent(&args(true, None), false).unwrap());
        assert!(consent(&args(false, Some("plan.json")), false).unwrap());
        assert!(!consent(&args(false, None), true).unwrap(), "a terminal asks");
    }

    #[test]
    fn an_apply_refuses_data_that_git_does_not_have() {
        let (fixture, remote) = repository("apply-uncommitted");
        let root = fixture.root();
        write(&root.join("data/env/local.toml"), "region = \"monaco\"\n");
        assert!(committed(&root).is_ok(), "local.toml is never in git");
        write(&root.join("data/env/live.toml"), "region = \"andorra\"\n");
        let before = keys(&fixture);
        let err = apply(&fixture, &remote, &Versioned).unwrap_err();
        let message = "data/ has changes that are not committed: data/env/live.toml";
        assert_eq!((err.code, err.message.as_str()), (Code::Usage, message));
        assert_eq!(keys(&fixture), before);
    }
}
