//! `obc data r2`: the R2 client as plumbing commands for scripts.

use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use clap::{Args, Subcommand};
use serde::Serialize;

use obc_data::r2::{Bucket, Credentials, Object, Upload, REMOVAL_LOG};

use crate::{cells, print_json, print_table, Failure};

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
    List {
        prefix: String,
        #[arg(long)]
        json: bool,
    },
    /// The objects of these keys that the bucket holds.
    Stat {
        #[arg(required = true)]
        keys: Vec<String>,
        #[arg(long)]
        json: bool,
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

pub fn run(r2: R2) -> Result<(), Failure> {
    let bucket = Bucket::from_env(if r2.fixtures { Credentials::Fixtures } else { Credentials::Main })?;
    match r2.action {
        Action::List { prefix, json } => print_objects(&bucket.list(&prefix)?, json),
        Action::Stat { keys, json } => print_objects(&bucket.stat(&keys)?.into_values().collect::<Vec<_>>(), json),
        Action::Get { key, file } => Ok(bucket.get(&key, &file)?),
        Action::Put { file, key, cache_control, content_type, immutable } => {
            let upload =
                Upload { cache_control: cache_control.as_deref(), content_type: content_type.as_deref(), immutable };
            bucket.put(&file, &key, &upload)?;
            bucket.verify(&file, &key)?;
            println!("{key}: uploaded and verified");
            Ok(())
        }
        Action::Delete { keys, prefix, reason, yes } => delete(&bucket, keys, prefix, &reason, yes),
    }
}

fn delete(bucket: &Bucket, keys: Vec<String>, prefix: Option<String>, reason: &str, yes: bool) -> Result<(), Failure> {
    if reason.trim().is_empty() {
        return Err(Failure { status: 2, message: format!("--reason is empty; {REMOVAL_LOG} keeps why objects go") });
    }
    let objects = match prefix {
        Some(prefix) => bucket.list(&prefix)?,
        None => {
            let found = bucket.stat(&keys)?;
            let missing: Vec<&str> = keys.iter().filter(|key| !found.contains_key(*key)).map(String::as_str).collect();
            if !missing.is_empty() {
                return Err(
                    format!("{} does not hold {}; nothing was deleted", bucket.describe(), missing.join(", ")).into()
                );
            }
            found.into_values().collect()
        }
    };
    if objects.is_empty() {
        return Err("that names no object in the bucket; nothing was deleted".into());
    }
    let bytes: u64 = objects.iter().map(|object| object.bytes).sum();
    println!("{}: {} object(s), {bytes} bytes to delete", bucket.describe(), objects.len());
    print_objects(&objects, false)?;
    if !yes {
        if !std::io::stdin().is_terminal() {
            return Err(Failure { status: 2, message: "without a terminal, pass --yes; nothing was deleted".into() });
        }
        print!("Delete {} object(s) from {}? [y/N] ", objects.len(), bucket.describe());
        std::io::stdout().flush().map_err(|e| e.to_string())?;
        let mut answer = String::new();
        std::io::stdin().lock().read_line(&mut answer).map_err(|e| e.to_string())?;
        if !matches!(answer.trim(), "y" | "Y" | "yes") {
            return Err("not confirmed; nothing was deleted".into());
        }
    }
    bucket.delete(&objects, reason)?;
    println!("deleted {} object(s); {REMOVAL_LOG} holds the record", objects.len());
    Ok(())
}

fn print_objects(objects: &[Object], json: bool) -> Result<(), Failure> {
    if json {
        #[derive(Serialize)]
        struct Listing<'a> {
            objects: &'a [Object],
        }
        return print_json(&Listing { objects });
    }
    let mut table = vec![cells(["KEY", "BYTES", "MODIFIED"])];
    table.extend(objects.iter().map(|o| vec![o.key.clone(), o.bytes.to_string(), o.modified.clone()]));
    print_table(&table);
    Ok(())
}
