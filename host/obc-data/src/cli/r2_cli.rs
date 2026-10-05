//! `obc data r2`: the R2 client as plumbing commands for scripts.

use std::path::{Path, PathBuf};

use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Serialize;

use crate::r2::{Bucket, Credentials, Object, Put, Upload, REMOVAL_LOG};

use super::api::confirm;
use super::{cells, print_json, table, Code, Error};

#[derive(Args)]
pub struct R2 {
    /// Use the fixture bucket (`OBC_FIXTURE_R2_*`) instead of `OBC_R2_*`.
    #[arg(long, global = true)]
    fixtures: bool,
    #[command(subcommand)]
    action: Action,
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
    },
}

/// The objects of a bucket that a command lists or deletes.
#[derive(Serialize, JsonSchema)]
pub struct Objects<'a> {
    /// The bucket or the local directory, never a credential.
    bucket: &'a str,
    objects: &'a [Object],
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
        Action::Delete { keys, prefix, reason, yes } => delete(&bucket, keys, prefix, &reason, yes, json),
    }
}

fn delete(
    bucket: &Bucket,
    keys: Vec<String>,
    prefix: Option<String>,
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
