//! `obc data source refresh ID`: fetch upstream's newest version of a source, so that the next
//! update of each target on this machine reads it. A source whose versions have no order, such as
//! a `digest`, moves no other way.

use std::path::Path;

use clap::Subcommand;
use schemars::JsonSchema;
use serde::Serialize;

use super::{fetched, find, print_json, registry, Code, Error};
use crate::fetch::{self, http::Http, Request};
use crate::store::Store;

#[derive(Subcommand)]
pub enum SourceCommand {
    /// Fetch upstream's newest version of a source: the next update of each target reads it.
    Refresh { id: String },
}

/// What `source refresh` did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Refreshed {
    pub source: String,
    /// Upstream's newest version, now in the store.
    pub version: String,
    /// The size of its files.
    pub bytes: u64,
}

pub fn run(root: &Path, command: SourceCommand, json: bool) -> Result<(), Error> {
    let SourceCommand::Refresh { id } = command;
    let refreshed = refresh(root, &Store::open()?, &Http::new(), &id)?;
    if json {
        return print_json(&refreshed);
    }
    println!("{} {}: the next update of each target reads it", refreshed.source, refreshed.version);
    Ok(())
}

/// Fetch the newest version of the source `id`, and record it as the version that a step list
/// reads.
pub(super) fn refresh(root: &Path, store: &Store, http: &Http, id: &str) -> Result<Refreshed, Error> {
    let registry = registry(root)?;
    let source = find(&registry, id)?;
    if let Some(name) = fetch::params(source).first() {
        return Err(Code::Usage
            .error(format!("source `{id}` is fetched per `{name}=`: it has no one newest version"))
            .fix(format!("Give the target a refresh policy: `obc data update TARGET --refresh {id}=DAYS`.")));
    }
    if !source.version.ordered() && source.start.is_some() {
        return Err(Code::Usage
            .error(format!("the version of `{id}` is its `start` in data/sources.toml"))
            .fix("Change the `start` there: the next update reads it."));
    }
    let request = Request { refresh: true, source, version: None, params: Vec::new() };
    let snapshot = fetched(source, fetch::fetch(root, store, http, &request))?;
    store.put_refreshed(id, &snapshot.version)?;
    let bytes = snapshot.files.iter().map(|file| file.size).sum();
    Ok(Refreshed { source: id.into(), version: snapshot.version, bytes })
}
