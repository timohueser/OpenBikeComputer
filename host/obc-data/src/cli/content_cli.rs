//! The manual content-source workflow used by the terminal interface and scripts.

use super::{print_json, Code, Error};
use crate::{
    content, operation,
    r2::{Bucket, Credentials},
    store::Store,
};
use clap::{Args, Subcommand};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Args)]
pub struct Content {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Save archive paths or URLs, source dates, and OSM coverage from a JSON configuration.
    Configure { file: PathBuf },
    /// Start a retained preparation with the saved configuration.
    Prepare,
    /// Show configuration, selected content and prepared versions without network access.
    Status,
    /// Review the immutable objects to publish. This command does not upload.
    Plan { snapshot: Option<String> },
    /// Publish the reviewed content version; scheduled bakes select its digest explicitly.
    Publish {
        snapshot: String,
        #[arg(long)]
        yes: bool,
    },
    /// Select a prepared version locally, or restore its manifest and lookup index from R2.
    Use {
        snapshot: String,
        #[arg(long)]
        from_r2: bool,
    },
}

#[derive(Serialize, JsonSchema)]
pub(super) struct ContentVersion {
    snapshot: String,
    records: u64,
    coverage: Value,
    origins: Value,
    bytes: u64,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct ContentStatus {
    configured: bool,
    selected: Option<content::Selected>,
    versions: Vec<ContentVersion>,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct ContentPlan {
    snapshot: String,
    manifest_key: String,
    objects: Vec<Value>,
    bytes: u64,
    coverage: Value,
    origins: Value,
    removes: Vec<String>,
}

pub(super) fn status(store: &Store) -> Result<ContentStatus, Error> {
    let selected = content::selected(store)?;
    let versions = store
        .snapshots(content::SOURCE)?
        .into_iter()
        .map(|snapshot| {
            let manifest = content::manifest(store, &snapshot.version)?;
            Ok(ContentVersion {
                snapshot: snapshot.version,
                records: manifest["records"].as_u64().ok_or("content manifest has no record count")?,
                coverage: manifest["coverage"].clone(),
                origins: manifest["origins"].clone(),
                bytes: manifest["files"].as_array().unwrap().iter().filter_map(|file| file["bytes"].as_u64()).sum(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(ContentStatus { configured: content::configured(store).is_ok(), selected, versions })
}

pub(super) fn start(root: &Path, store: &Store) -> Result<super::operation_cli::Handle, Error> {
    let request = operation::Request {
        kind: operation::Kind::ContentPrepare,
        env: "content".into(),
        only: vec![],
        moves: vec![],
        plan: None,
        dev: None,
        content: Some(content::configured(store)?),
    };
    super::operation_cli::start(root, store, request, None)
}

pub(super) fn plan(store: &Store, digest: &str) -> Result<ContentPlan, Error> {
    let manifest = content::manifest(store, digest)?;
    Ok(ContentPlan {
        snapshot: digest.into(),
        manifest_key: format!("content/manifests/{digest}.json"),
        objects: manifest["files"].as_array().unwrap().clone(),
        bytes: manifest["files"].as_array().unwrap().iter().filter_map(|file| file["bytes"].as_u64()).sum(),
        coverage: manifest["coverage"].clone(),
        origins: manifest["origins"].clone(),
        removes: vec![],
    })
}

pub(super) fn publish(root: &Path, store: &Store, digest: &str) -> Result<super::operation_cli::Handle, Error> {
    plan(store, digest)?;
    Bucket::from_env(Credentials::Main).map_err(|e| Code::Blocked.error(e))?;
    let request = operation::Request {
        kind: operation::Kind::ContentPublish,
        env: "content".into(),
        only: vec![],
        moves: vec![],
        plan: None,
        dev: None,
        content: Some(content::Request { config: json!({"snapshot":digest}) }),
    };
    super::operation_cli::start(root, store, request, None)
}

fn output(value: &(impl Serialize + JsonSchema), json: bool) -> Result<(), Error> {
    if json {
        print_json(value)
    } else {
        println!("{}", serde_json::to_string_pretty(value).map_err(|e| e.to_string())?);
        Ok(())
    }
}

pub fn run(root: &Path, store: &Store, args: Content, json: bool) -> Result<(), Error> {
    match args.action {
        Action::Configure { file } => output(&content::configure(store, &file)?, json),
        Action::Prepare => super::operation_cli::print_handle(&start(root, store)?, json),
        Action::Status => output(&status(store)?, json),
        Action::Plan { snapshot } => {
            let snapshot = snapshot
                .or(content::selected(store)?.map(|selected| selected.sha256))
                .ok_or_else(|| Code::Usage.error("prepare or select a content snapshot first"))?;
            output(&plan(store, &snapshot)?, json)
        }
        Action::Publish { snapshot, yes } => {
            let review = plan(store, &snapshot)?;
            eprintln!(
                "Publish content {snapshot}: {} bytes; {} objects; no removals",
                review.bytes,
                review.objects.len() + 1
            );
            super::api::confirm("Publish this immutable content version to R2?", yes)?;
            super::operation_cli::print_handle(&publish(root, store, &snapshot)?, json)
        }
        Action::Use { snapshot, from_r2 } => {
            let selected = if from_r2 {
                let bucket = Bucket::from_env(Credentials::Main).map_err(|e| Code::Blocked.error(e))?;
                content::restore(store, &bucket, &snapshot)?
            } else {
                content::use_local(store, &snapshot)?
            };
            output(&selected, json)
        }
    }
}
