//! `obc data r2`: the R2 client as plumbing commands for scripts, and the legacy keys of a target.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Serialize;

use crate::r2::{
    legacy as legacy_key, owned as release_key, refuse_older_publish, Bucket, Credentials, Object, Put, Upload,
    REMOVAL_LOG,
};
use crate::release;
use crate::sources::FetchKind;
use crate::store::{sha256_hex, Store};
use crate::target::Target;

use super::api::confirm;
use super::{cells, print_json, table, Code, Error};

/// The folders of the legacy pipelines that `r2 legacy` lists, sorted. Without `--prefix`, it lists
/// the first two: `reference/v1` holds terrain until a version of `terrain` holds each of its models.
const LEGACY: [&str; 3] = ["cell-catalog", "planner", "reference/v1"];
/// The index of `reference/v1/`: its `sources` names the key of each country that it holds.
const INDEX: &str = "reference/v1/index.json";

#[derive(serde::Deserialize)]
struct Index {
    sources: BTreeMap<String, serde::de::IgnoredAny>,
}

#[derive(Args)]
pub struct R2 {
    /// Use the fixture bucket (`OBC_FIXTURE_R2_*`) instead of `OBC_R2_*`.
    #[arg(long, global = true)]
    fixtures: bool,
    #[command(subcommand)]
    action: Action,
}

#[derive(Args)]
struct LegacyArgs {
    target: String,
    /// List this folder instead of `cell-catalog` and `planner`.
    #[arg(long, value_parser = LEGACY)]
    prefix: Option<String>,
    /// List the keys again and delete them when their digest is `--expect`.
    #[arg(long, requires_all = ["expect", "reason"])]
    delete: bool,
    /// The digest of the list that the owner approved.
    #[arg(long, requires = "delete")]
    expect: Option<String>,
    /// Why the keys go; removed.jsonl keeps it.
    #[arg(long, requires = "delete")]
    reason: Option<String>,
    /// Do not ask. Required without a terminal.
    #[arg(long, requires = "delete")]
    yes: bool,
}

#[derive(Subcommand)]
enum Action {
    /// Every object under a prefix.
    List { prefix: String },
    /// The objects of these keys that the bucket holds.
    Stat {
        #[arg(required = true)]
        keys: Vec<String>,
    },
    /// Download one object.
    Get { key: String, file: PathBuf },
    /// Upload a file with a checksum and the headers given, then verify it.
    Put {
        file: PathBuf,
        key: String,
        #[arg(long)]
        cache_control: Option<String>,
        #[arg(long)]
        content_type: Option<String>,
        /// Never replace an object at the key; one with other bytes is an error.
        #[arg(long)]
        immutable: bool,
    },
    /// Show the plan, ask, append to removed.jsonl, then delete.
    Delete {
        #[arg(required_unless_present = "prefix")]
        keys: Vec<String>,
        /// Delete every object under this prefix instead.
        #[arg(long, conflicts_with = "keys")]
        prefix: Option<String>,
        /// Why the objects go; removed.jsonl keeps it.
        #[arg(long)]
        reason: String,
        /// Do not ask. Required without a terminal.
        #[arg(long)]
        yes: bool,
        /// Allow keys under `cell-catalog/`, `planner/` and `reference/`, of the legacy pipelines.
        #[arg(long)]
        legacy: bool,
    },
    /// The keys under `cell-catalog/` and `planner/` that the target does not use, and their
    /// digest; with `--delete`, delete them.
    Legacy(LegacyArgs),
}

/// The objects of a bucket that a command lists or deletes.
#[derive(Serialize, JsonSchema)]
pub struct Objects<'a> {
    /// The bucket or the local directory, never a credential.
    bucket: &'a str,
    objects: &'a [Object],
}

/// The keys of the legacy pipelines that a target does not use.
#[derive(Serialize, JsonSchema)]
pub struct Legacy {
    target: String,
    /// The keys of each folder, sorted.
    folders: BTreeMap<String, Vec<LegacyKey>>,
    /// The SHA-256 of the lines `<bytes>  <key>\n` of the keys of every folder, sorted by key.
    digest: String,
}

#[derive(Serialize, JsonSchema)]
pub struct LegacyKey {
    key: String,
    bytes: u64,
}

#[derive(Serialize, JsonSchema)]
pub struct Downloaded<'a> {
    key: &'a str,
    file: &'a Path,
}

#[derive(Serialize, JsonSchema)]
pub struct Uploaded<'a> {
    key: &'a str,
    /// `false` when an immutable key already held these bytes.
    uploaded: bool,
}

pub fn run(r2: R2, json: bool) -> Result<(), Error> {
    if let Action::Legacy(args) = r2.action {
        if r2.fixtures {
            return Err(Code::Usage.error("`legacy` reads the bucket of its target, never the fixture bucket"));
        }
        let (root, store) = (super::root()?, Store::open()?);
        let target = crate::target::find(&root, &args.target, &store).map_err(|e| Code::Usage.error(e))?;
        return legacy(&store, &target, args, json);
    }
    let failed = |e| Code::R2Failed.error(e);
    let bucket = Bucket::from_env(if r2.fixtures { Credentials::Fixtures } else { Credentials::Main })
        .map_err(|e| Code::Blocked.error(e))?;
    match r2.action {
        Action::List { prefix } => print_objects(&bucket, &bucket.list(&prefix).map_err(failed)?, json),
        Action::Stat { keys } => {
            print_objects(&bucket, &bucket.stat(&keys).map_err(failed)?.into_values().collect::<Vec<_>>(), json)
        }
        Action::Get { key, file } => {
            bucket.get(&key, &file).map_err(failed)?;
            if json {
                return print_json(&Downloaded { key: &key, file: &file });
            }
            Ok(())
        }
        Action::Put { file, key, cache_control, content_type, immutable } => {
            refuse_owned(&bucket, std::slice::from_ref(&key)).map_err(owned)?;
            let upload =
                Upload { cache_control: cache_control.as_deref(), content_type: content_type.as_deref(), immutable };
            let uploaded = bucket.put(&file, &key, &upload).map_err(failed)? == Put::Uploaded;
            if uploaded {
                bucket.verify(&file, &key).map_err(|e| Code::VerifyFailed.error(e))?;
            }
            if json {
                return print_json(&Uploaded { key: &key, uploaded });
            }
            if uploaded {
                println!("{key}: uploaded and verified");
            } else {
                println!("{key}: already holds these bytes; nothing uploaded");
            }
            Ok(())
        }
        Action::Delete { keys, prefix, reason, yes, legacy } => {
            delete(&bucket, (keys, prefix, legacy), None, &reason, yes, json)
        }
        Action::Legacy(_) => unreachable!("`legacy` returns above"),
    }
}

/// Refuse `keys` when a release of `obc data` writes one of them and the bucket has a release.
fn refuse_owned(bucket: &Bucket, keys: &[String]) -> Result<(), String> {
    match keys.iter().any(|key| release_key(key)) {
        true => refuse_older_publish(bucket),
        false => Ok(()),
    }
}

/// Refuse a key under `sources/<id>/` of a prepared source while the home of `id` in the archive
/// roots `roots` lacks a version that `sources/<id>/versions.json` of the bucket lists with its
/// manifest SHA-256: the archive is then the only copy of a version that is not there yet.
fn refuse_uncopied(bucket: &Bucket, keys: &[String], roots: &[PathBuf]) -> Result<(), Error> {
    let prepared = |id: &&str| {
        crate::sources::all().iter().any(|source| source.id == *id && source.fetch.kind == FetchKind::Prepared)
    };
    let ids: BTreeSet<&str> =
        keys.iter().filter_map(|key| Some(key.strip_prefix("sources/")?.split_once('/')?.0)).filter(prepared).collect();
    for id in ids {
        let refused = |text: String| {
            Code::Usage
                .error(format!("{text}; nothing was deleted"))
                .fix(format!("Copy the versions first: `obc data source copy {id} --from TARGET`."))
        };
        let Some(bytes) = bucket.read(&format!("sources/{id}/versions.json")).map_err(|e| Code::R2Failed.error(e))?
        else {
            continue;
        };
        let theirs: Vec<crate::prepared::Version> = serde_json::from_slice(&bytes)
            .map_err(|e| refused(format!("sources/{id}/versions.json of the bucket: {e}")))?;
        let ours = match crate::prepared::home(roots, id) {
            Some(root) => crate::prepared::versions(&crate::prepared::folder(root, id), id)?,
            None => Vec::new(),
        };
        let copied = |entry: &crate::prepared::Version| {
            ours.iter().any(|ours| ours.version == entry.version && ours.manifest_sha256 == entry.manifest_sha256)
        };
        if let Some(entry) = theirs.iter().find(|entry| !copied(entry)) {
            return Err(refused(format!("the archive has no copy of {id}@{} of the bucket", entry.version)));
        }
    }
    Ok(())
}

/// A refusal of a key that only an update writes.
fn owned(message: String) -> Error {
    Code::R2Failed.error(message).fix("Once a bucket has a release, only `obc data update` changes its keys.")
}

fn delete(
    bucket: &Bucket,
    (keys, prefix, allow_legacy): (Vec<String>, Option<String>, bool),
    summary: Option<String>,
    reason: &str,
    yes: bool,
    json: bool,
) -> Result<(), Error> {
    let failed = |e| Code::R2Failed.error(e);
    if reason.trim().is_empty() {
        return Err(Code::Usage.error(format!("--reason is empty; {REMOVAL_LOG} keeps why objects go")));
    }
    let keys = match prefix {
        Some(prefix) => bucket.list(&prefix).map_err(failed)?.into_iter().map(|object| object.key).collect(),
        None => keys,
    };
    refuse_owned(bucket, &keys).map_err(|e| owned(format!("{e}; nothing was deleted")))?;
    refuse_uncopied(bucket, &keys, &crate::fetch::archive::roots())?;
    if let Some(key) = keys.iter().find(|key| legacy_key(key)).filter(|_| !allow_legacy) {
        return Err(Code::Usage
            .error(format!("{key} is a key of the legacy pipelines; nothing was deleted"))
            .fix("Legacy keys go only after the cutover, from the owner-approved list, with --legacy."));
    }
    let objects = bucket.plan_delete(&keys).map_err(|e| failed(format!("{e}; nothing was deleted")))?;
    let bytes: u64 = objects.iter().map(|object| object.bytes).sum();
    let each = summary.unwrap_or_else(|| listing(bucket, &objects));
    let plan = format!("{} object(s), {bytes} bytes to delete\n{each}", objects.len());
    // With `--json`, standard output holds only the JSON.
    if json {
        eprint!("{plan}");
    } else {
        print!("{plan}");
    }
    confirm(&format!("Delete {} object(s) from {}?", objects.len(), bucket.describe()), yes)?;
    bucket.delete(&objects, reason).map_err(failed)?;
    if json {
        return print_json(&Objects { bucket: bucket.describe(), objects: &objects });
    }
    println!("deleted {} object(s); {REMOVAL_LOG} holds the record", objects.len());
    Ok(())
}

/// List the legacy keys of `target`; with `--delete`, ask, then delete them in a detached run, so
/// a long delete goes on without its terminal or page.
fn legacy(store: &Store, target: &Target, args: LegacyArgs, json: bool) -> Result<(), Error> {
    if let Some(done) = super::api::detached(store, |_| remove(store, target, &args), |()| Ok(())) {
        return done;
    }
    let listed = listed(target, &args)?;
    if !args.delete {
        if json {
            return print_json(&listed);
        }
        print!("{}", summary(&listed));
        return Ok(());
    }
    // With `--json`, standard output holds only the JSON.
    let text = summary(&listed);
    if json {
        eprint!("{text}");
    } else {
        print!("{text}");
    }
    let keys = listed.folders.values().flatten().count();
    confirm(&format!("Delete {keys} legacy key(s) from `{}`?", args.target), args.yes)?;
    if store.is_locked(&deleting(&args.target))? {
        return Err(busy(&args.target));
    }
    let mut run = vec!["r2".into(), "legacy".into(), args.target.clone()];
    run.extend(args.prefix.iter().flat_map(|prefix| ["--prefix".into(), prefix.clone()]));
    run.extend(["--delete", "--expect", &listed.digest, "--reason"].map(String::from));
    run.extend([args.reason.clone().unwrap_or_default(), "--yes".into()]);
    let run = crate::engine::runs::detach(store, &run)?;
    if json {
        return print_json(&super::api::Detached { run });
    }
    eprintln!("obc data: run {run}");
    super::runs_cli::follow(store, &run, false)
}

/// The legacy keys of `target`, and with `--delete` the refusal of another list than `--expect`.
fn listed(target: &Target, args: &LegacyArgs) -> Result<Legacy, Error> {
    let listed = unused((&args.target, target), &crate::fetch::archive::roots(), args.prefix.as_deref())?;
    if args.delete {
        let what = format!("r2 legacy {}", args.target);
        super::update_cli::expect(&what, &listed.digest, Some(args.expect.as_deref().unwrap_or_default()))
            .map_err(|e| e.fix("List the keys again: run the same command without `--delete`."))?;
    }
    Ok(listed)
}

/// The lock of the run that deletes the legacy keys of `target`: one at a time.
fn deleting(target: &str) -> String {
    format!("r2-legacy-{target}")
}

fn busy(target: &str) -> Error {
    Code::Blocked
        .error(format!("another run deletes the legacy keys of `{target}`"))
        .fix("Wait for it to end: `obc data runs` shows it.")
}

/// In the run: list the keys again, and delete them while their digest is `--expect`.
fn remove(store: &Store, target: &Target, args: &LegacyArgs) -> Result<(), Error> {
    let _deleting = store.try_lock(&deleting(&args.target))?.ok_or_else(|| busy(&args.target))?;
    let listed = listed(target, args)?;
    let bucket = match target {
        Target::Dir(dir) => Bucket::local(dir),
        Target::R2(remote) => remote.writer()?.bucket()?,
    };
    let keys: Vec<String> = listed.folders.values().flatten().map(|k| k.key.clone()).collect();
    if keys.is_empty() {
        println!("no legacy key is left; nothing was deleted");
        return Ok(());
    }
    delete(&bucket, (keys, None, true), Some(summary(&listed)), args.reason.as_deref().unwrap_or_default(), true, false)
}

/// The keys under `cell-catalog/` and `planner/`, or under `prefix`, that `obc data` does not
/// write; `Release::check` keeps every other key of a release out of these folders. `roots` are the
/// archive roots, where `terrain` is. See the spec for when it refuses.
fn unused((name, target): (&str, &Target), roots: &[PathBuf], prefix: Option<&str>) -> Result<Legacy, Error> {
    if release::serving(target)?.is_none() {
        return Err(Code::Blocked
            .error(format!("target `{name}` serves no release; nothing is legacy before the cutover"))
            .fix(format!("Run `obc data update {name}` first.")));
    }
    if prefix == Some(LEGACY[2]) {
        let index: Option<Index> = target.get(INDEX)?.and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let index = index.ok_or_else(|| Code::Blocked.error(format!("{INDEX} is missing or does not parse")))?;
        // The index of the newest version of `terrain` names each model that it holds.
        let mut held = None;
        if let Some(root) = crate::prepared::home(roots, "terrain") {
            let folder = crate::prepared::folder(root, "terrain");
            if let Some(newest) = crate::prepared::versions(&folder, "terrain")?.pop() {
                held = folder.get(&format!("{}/index.json", newest.version))?;
            }
        }
        let held: Option<Index> = held.and_then(|bytes| serde_json::from_slice(&bytes).ok());
        let held = held.map(|held| held.sources).unwrap_or_default();
        let missing: Vec<String> = index.sources.into_keys().filter(|key| !held.contains_key(key)).collect();
        if !missing.is_empty() {
            return Err(Code::Blocked
                .error(format!("`terrain` on `{name}` lacks {} sources of {INDEX}", missing.len()))
                .fix(format!("Add their areas to `terrain` first: {}.", missing.join(", "))));
        }
    }
    let mut folders: BTreeMap<String, Vec<LegacyKey>> = BTreeMap::new();
    for prefix in prefix.map_or(LEGACY[..2].to_vec(), |prefix| vec![prefix]) {
        let mut keys = target.list_under(prefix)?;
        keys.retain(|listed| !release_key(&listed.key));
        folders.insert(prefix.into(), keys.into_iter().map(|l| LegacyKey { key: l.key, bytes: l.size }).collect());
    }
    // Each listing is sorted, so the lines are sorted by key.
    let lines: String = folders.values().flatten().map(|k| format!("{}  {}\n", k.bytes, k.key)).collect();
    Ok(Legacy { target: name.into(), folders, digest: sha256_hex(lines.as_bytes()) })
}

/// The keys and bytes of each folder, and the digest.
fn summary(listed: &Legacy) -> String {
    let mut rows = vec![cells(["PREFIX", "KEYS", "BYTES"])];
    for (prefix, keys) in &listed.folders {
        rows.push(vec![prefix.clone(), keys.len().to_string(), keys.iter().map(|k| k.bytes).sum::<u64>().to_string()]);
    }
    format!("{}\n{}digest {}\n", listed.target, table(&rows), listed.digest)
}

fn print_objects(bucket: &Bucket, objects: &[Object], json: bool) -> Result<(), Error> {
    if json {
        return print_json(&Objects { bucket: bucket.describe(), objects });
    }
    print!("{}", listing(bucket, objects));
    Ok(())
}

fn listing(bucket: &Bucket, objects: &[Object]) -> String {
    let mut rows = vec![cells(["KEY", "BYTES", "MODIFIED"])];
    rows.extend(objects.iter().map(|o| vec![o.key.clone(), o.bytes.to_string(), o.modified.clone()]));
    format!("{}\n{}", bucket.describe(), table(&rows))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legacy_key_goes_only_with_legacy() {
        let scratch = crate::store::tests::Scratch::new("r2-legacy");
        let cell = scratch.0.join("cell-catalog/a.obcm");
        crate::engine::tests::write(&cell, "a");
        let bucket = Bucket::local(&scratch.0);
        let keys = || (vec!["cell-catalog/a.obcm".to_string()], None, false);
        let refused = delete(&bucket, keys(), None, "test", true, true).unwrap_err();
        assert!(refused.message.starts_with("cell-catalog/a.obcm is a key of the legacy pipelines") && cell.exists());
        delete(&bucket, (keys().0, None, true), None, "test", true, true).unwrap();
        assert!(!cell.exists());
    }

    #[test]
    fn a_delete_of_a_prepared_source_needs_its_versions_in_the_archive() {
        let scratch = crate::store::tests::Scratch::new("r2-uncopied");
        let (bucket, archive) = (Bucket::local(&scratch.0.join("bucket")), scratch.0.join("archive"));
        let listed = r#"[{"version": "2026-10-28", "manifest_sha256": "aa", "format": 3, "bytes": 1, "time": ""}]"#;
        let put = |dir: &Path, bytes: &str| {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("versions.json"), bytes).unwrap();
        };
        put(&scratch.0.join("bucket/sources/terrain"), listed);
        let keys = ["sources/terrain/versions.json".to_string(), "sources/era5-land/a.nc".to_string()];
        let refused = refuse_uncopied(&bucket, &keys, std::slice::from_ref(&archive)).unwrap_err();
        assert!(refused.message.contains("no copy of terrain@2026-10-28"), "{}", refused.message);
        put(&crate::prepared::path(&archive, "terrain"), &listed.replace("\"aa\"", "\"bb\""));
        assert!(refuse_uncopied(&bucket, &keys, std::slice::from_ref(&archive)).is_err(), "another manifest");
        put(&crate::prepared::path(&archive, "terrain"), listed);
        refuse_uncopied(&bucket, &keys, &[archive]).unwrap();
        refuse_uncopied(&bucket, &keys[1..], &[]).unwrap();
    }

    /// Keys of the legacy pipelines that `obc data` does not write.
    const OLD: [&str; 3] = ["cell-catalog/cells/a.obcm", "planner/objects/a", "reference/v1/a.tif"];

    /// A dir target that serves a release with a rollback, and has `planner/next.json` and [`OLD`].
    fn served(name: &str) -> (crate::store::tests::Scratch, Target) {
        let scratch = crate::store::tests::Scratch::new(name);
        let target = Target::Dir(scratch.0.join("local"));
        for key in OLD.into_iter().chain([release::NEXT]) {
            target.put(key, b"{}", &crate::target::Condition::Any).unwrap();
        }
        assert_eq!(unused(("local", &target), &[], None).err().map(|e| e.code), Some(Code::Blocked), "no cutover");
        for device in ["device a", "device b"] {
            let (id, _) = crate::serve::tests::release_at(&target, device, device.as_bytes(), 1, "https://api.test");
            assert!(release::switch(&target, release::serving(&target).unwrap().as_ref(), &id).unwrap());
        }
        (scratch, target)
    }

    #[test]
    fn legacy_lists_only_the_keys_that_obc_data_does_not_write() {
        let (scratch, target) = served("r2-legacy-list");
        let serving = release::serving(&target).unwrap().unwrap().0;
        assert!(serving.rollback.is_some() && target.get(release::POINTERS[1]).unwrap().is_some());
        let roots = [scratch.0.join("archive")];
        let keys = |prefix| {
            unused(("local", &target), &roots, prefix).map(|l| l.folders.into_values().flatten().map(|k| k.key))
        };
        assert_eq!(keys(None).unwrap().collect::<Vec<_>>(), OLD[..2]);

        // `reference/v1` goes only when the newest version of `terrain` holds each source of its index.
        let reference = || keys(Some("reference/v1")).map(|keys| keys.collect::<Vec<_>>());
        assert_eq!(reference().err().map(|e| e.code), Some(Code::Blocked), "no index.json");
        let put = |key, bytes: &str| target.put(key, bytes.as_bytes(), &crate::target::Condition::Any).unwrap();
        put(INDEX, r#"{"sources": {"ch": {"product": "swissALTI3D"}}}"#);
        assert!(reference().unwrap_err().fix.ends_with(": ch."));
        let version = r#"[{"version": "2026-10-01", "manifest_sha256": "", "format": 3, "bytes": 1, "time": ""}]"#;
        let terrain = crate::prepared::folder(&roots[0], "terrain");
        let held = |key, bytes: &str| terrain.put(key, bytes.as_bytes(), &crate::target::Condition::Any).unwrap();
        held("versions.json", version);
        held("2026-10-01/index.json", r#"{"sources": {"ch": {"product": "swissALTI3D"}}}"#);
        assert_eq!(reference().unwrap(), [OLD[2], INDEX]);
    }

    #[test]
    fn a_legacy_delete_needs_the_digest_of_the_list() {
        let (scratch, target) = served("r2-legacy-delete");
        let args = |expect: &str| LegacyArgs {
            target: "local".into(),
            prefix: Some("planner".into()),
            delete: true,
            expect: Some(expect.into()),
            reason: Some("test".into()),
            yes: true,
        };
        let store = Store::open_at(scratch.0.join("store")).unwrap();
        let before = target.list().unwrap();
        let refused = remove(&store, &target, &args(&"0".repeat(64))).unwrap_err();
        assert!(refused.code == Code::Usage && target.list().unwrap() == before);
        let held = store.try_lock(&deleting("local")).unwrap().unwrap();
        assert_eq!(remove(&store, &target, &args(&"0".repeat(64))).unwrap_err().code, Code::Blocked, "one at a time");
        drop(held);
        let next = (vec![release::NEXT.to_string()], None, true);
        let refused = delete(&Bucket::local(&scratch.0.join("local")), next, None, "test", true, true).unwrap_err();
        assert!(refused.message.starts_with("the bucket has `release.json`"), "{}", refused.message);
        let digest = unused(("local", &target), &[], Some("planner")).unwrap().digest;
        remove(&store, &target, &args(&digest)).unwrap();
        let after = target.list().unwrap().into_iter().map(|listed| listed.key).collect::<Vec<_>>();
        let gone: Vec<_> = before.into_iter().map(|listed| listed.key).filter(|key| !after.contains(key)).collect();
        assert_eq!(gone, [OLD[1]]);
        remove(&store, &target, &args(&sha256_hex(b""))).unwrap();
    }
}
