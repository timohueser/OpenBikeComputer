//! `obc data r2`: the R2 client as plumbing commands for scripts, and the legacy keys of a target.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Serialize;

use crate::r2::{
    legacy as legacy_key, owned as release_key, refuse_older_publish, Bucket, Credentials, Object, Put, Upload,
    REMOVAL_LOG,
};

use crate::release::{self, Release, NEXT, POINTERS};
use crate::store::{sha256_hex, Store};
use crate::target::Target;

use super::api::confirm;
use super::{cells, print_json, table, Code, Error};

/// The folders of the legacy pipelines that `r2 legacy` lists, sorted. `reference/v1/` goes later
/// than the other two, so `--prefix` takes one.
const LEGACY: [&str; 3] = ["cell-catalog/", "planner/", "reference/v1/"];

#[derive(Args)]
pub struct R2 {
    /// Use the fixture bucket (`OBC_FIXTURE_R2_*`) instead of `OBC_R2_*`.
    #[arg(long, global = true)]
    fixtures: bool,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    #[command(flatten)]
    Bucket(Action),
    /// The keys under `cell-catalog/`, `planner/` and `reference/v1/` that the target does not
    /// use, and their digest; with `--delete`, delete them.
    Legacy(LegacyArgs),
}

#[derive(Args)]
struct LegacyArgs {
    target: String,
    /// List only this folder.
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
    folders: Vec<Folder>,
    /// The SHA-256 of the lines `<bytes>  <key>` of the keys of every folder, sorted by key.
    digest: String,
}

#[derive(Serialize, JsonSchema)]
pub struct Folder {
    prefix: String,
    bytes: u64,
    keys: Vec<String>,
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
    let action = match r2.command {
        Command::Bucket(action) => action,
        Command::Legacy(args) => {
            let target = crate::target::find(&super::root()?, &args.target, &Store::open()?)
                .map_err(|e| Code::Usage.error(e))?;
            return legacy(&target, args, json);
        }
    };
    let failed = |e| Code::R2Failed.error(e);
    let bucket = Bucket::from_env(if r2.fixtures { Credentials::Fixtures } else { Credentials::Main })
        .map_err(|e| Code::Blocked.error(e))?;
    match action {
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
            delete(&bucket, (keys, prefix, legacy), &reason, yes, json)
        }
    }
}

/// Refuse `keys` when a release of `obc data` writes one of them and the bucket has a release.
fn refuse_owned(bucket: &Bucket, keys: &[String]) -> Result<(), String> {
    match keys.iter().any(|key| release_key(key)) {
        true => refuse_older_publish(bucket),
        false => Ok(()),
    }
}

/// A refusal of a key that only an update writes.
fn owned(message: String) -> Error {
    Code::R2Failed.error(message).fix("Once a bucket has a release, only `obc data update` changes its keys.")
}

fn delete(
    bucket: &Bucket,
    (keys, prefix, allow_legacy): (Vec<String>, Option<String>, bool),
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
    if let Some(key) = keys.iter().find(|key| legacy_key(key)).filter(|_| !allow_legacy) {
        return Err(Code::Usage
            .error(format!("{key} is a key of the legacy pipelines; nothing was deleted"))
            .fix("Legacy keys go only after the cutover, from the owner-approved list, with --legacy."));
    }
    let objects = bucket.plan_delete(&keys).map_err(|e| failed(format!("{e}; nothing was deleted")))?;
    let bytes: u64 = objects.iter().map(|object| object.bytes).sum();
    let plan = format!("{} object(s), {bytes} bytes to delete\n{}", objects.len(), listing(bucket, &objects));
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

fn legacy(target: &Target, args: LegacyArgs, json: bool) -> Result<(), Error> {
    let listed = unused(&args.target, target, args.prefix.as_deref())?;
    if !args.delete {
        if json {
            return print_json(&listed);
        }
        let mut rows = vec![cells(["PREFIX", "KEYS", "BYTES"])];
        rows.extend(
            listed.folders.iter().map(|f| vec![f.prefix.clone(), f.keys.len().to_string(), f.bytes.to_string()]),
        );
        print!("{}\n{}digest {}\n", listed.target, table(&rows), listed.digest);
        return Ok(());
    }
    if args.expect.as_deref() != Some(listed.digest.as_str()) {
        return Err(Code::Blocked
            .error(format!("the list has the digest {}, not the one expected; nothing was deleted", listed.digest))
            .fix("List the keys again, and delete the list that the owner approves."));
    }
    if let Some(holder) = crate::lock::holder(target)? {
        return Err(Code::Blocked.error(format!("{}; nothing was deleted", crate::lock::held_by(&holder))));
    }
    let bucket = match target {
        Target::Dir(dir) => Bucket::local(dir),
        Target::R2(remote) => remote.writer()?.bucket()?,
    };
    let keys = listed.folders.into_iter().flat_map(|folder| folder.keys).collect();
    delete(&bucket, (keys, None, true), &args.reason.unwrap_or_default(), args.yes, json)
}

/// The keys under the folders of [`LEGACY`], or under `prefix`, that `target` does not use: that
/// neither its serving nor its rollback release names, and that are no pointer and not [`NEXT`].
/// It refuses before the cutover, while the target serves no release.
fn unused(name: &str, target: &Target, prefix: Option<&str>) -> Result<Legacy, Error> {
    let Some((serving, _)) = release::serving(target)? else {
        return Err(Code::Blocked
            .error(format!("target `{name}` serves no release; nothing is legacy before the cutover"))
            .fix(format!("Run `obc data update {name}` first.")));
    };
    let mut used: BTreeSet<String> = POINTERS.iter().chain([&NEXT]).map(|key| key.to_string()).collect();
    for role in std::iter::once(&serving.serving).chain(&serving.rollback) {
        let release = Release::read(target, &role.id)?;
        used.extend(release.files().into_iter().chain(release.pointers).map(|file| file.key));
    }
    let (mut folders, mut lines) = (Vec::new(), String::new());
    for prefix in LEGACY.into_iter().filter(|folder| prefix.is_none_or(|only| only == *folder)) {
        let mut keys = target.list_under(prefix.trim_end_matches('/'))?;
        keys.retain(|listed| !used.contains(&listed.key));
        // LEGACY is sorted and each listing is sorted, so the lines are sorted by key.
        lines.extend(keys.iter().map(|listed| format!("{}  {}\n", listed.size, listed.key)));
        let bytes = keys.iter().map(|listed| listed.size).sum();
        let keys: Vec<String> = keys.into_iter().map(|listed| listed.key).collect();
        folders.push(Folder { prefix: prefix.into(), bytes, keys });
    }
    Ok(Legacy { target: name.into(), folders, digest: sha256_hex(lines.as_bytes()) })
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
        let refused = delete(&bucket, keys(), "test", true, true).unwrap_err();
        assert!(refused.message.starts_with("cell-catalog/a.obcm is a key of the legacy pipelines") && cell.exists());
        delete(&bucket, (keys().0, None, true), "test", true, true).unwrap();
        assert!(!cell.exists());
    }

    /// The keys of the legacy pipelines that a target with a serving and a rollback release does
    /// not use.
    const OLD: [&str; 3] = ["cell-catalog/cells/a.obcm", "planner/objects/a", "reference/v1/a.tif"];

    /// A dir target that serves a release with a rollback, has `planner/next.json` and the keys
    /// [`OLD`].
    fn served(name: &str) -> (crate::store::tests::Scratch, Target) {
        let scratch = crate::store::tests::Scratch::new(name);
        let target = Target::Dir(scratch.0.join("local"));
        for key in OLD.into_iter().chain([NEXT]) {
            target.put(key, b"{}", &crate::target::Condition::Any).unwrap();
        }
        assert_eq!(unused("local", &target, None).err().map(|e| e.code), Some(Code::Blocked), "no cutover");
        for device in ["device a", "device b"] {
            let (id, _) = crate::serve::tests::release_at(&target, device, device.as_bytes(), 1, "https://api.test");
            assert!(release::switch(&target, release::serving(&target).unwrap().as_ref(), &id).unwrap());
        }
        (scratch, target)
    }

    #[test]
    fn legacy_lists_only_the_keys_that_no_release_pointer_or_next_uses() {
        let (_scratch, target) = served("r2-legacy-list");
        let serving = release::serving(&target).unwrap().unwrap().0;
        assert!(serving.rollback.is_some() && target.get(POINTERS[1]).unwrap().is_some());
        let keys = |prefix| unused("local", &target, prefix).unwrap().folders.into_iter().flat_map(|f| f.keys);
        assert_eq!(keys(None).collect::<Vec<_>>(), OLD);
        assert_eq!(keys(Some("reference/v1/")).collect::<Vec<_>>(), [OLD[2]]);
    }

    #[test]
    fn a_legacy_delete_needs_the_digest_of_the_list() {
        let (_scratch, target) = served("r2-legacy-delete");
        let args = |expect: &str| LegacyArgs {
            target: "local".into(),
            prefix: Some("planner/".into()),
            delete: true,
            expect: Some(expect.into()),
            reason: Some("test".into()),
            yes: true,
        };
        let before = target.list().unwrap();
        let refused = legacy(&target, args(&"0".repeat(64)), true).unwrap_err();
        assert!(refused.message.ends_with("nothing was deleted") && target.list().unwrap() == before);
        let digest = unused("local", &target, Some("planner/")).unwrap().digest;
        legacy(&target, args(&digest), true).unwrap();
        let after = target.list().unwrap().into_iter().map(|listed| listed.key).collect::<Vec<_>>();
        let gone: Vec<_> = before.into_iter().map(|listed| listed.key).filter(|key| !after.contains(key)).collect();
        assert_eq!(gone, [OLD[1]]);
    }
}
