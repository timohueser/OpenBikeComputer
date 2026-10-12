//! `obc data source refresh ID`: fetch upstream's newest version of a `digest` source, so that the
//! next update of each target that serves the version of now reads it. A source whose versions
//! have no order moves no other way.

use std::collections::BTreeSet;
use std::path::Path;

use clap::Subcommand;
use schemars::JsonSchema;
use serde::Serialize;

use super::{fetched, find, print_json, registry, releases, Code, Error};
use crate::fetch::{self, http::Http, Request};
use crate::sources::VersionScheme;
use crate::store::{RefreshedVersion, Store};

#[derive(Subcommand)]
pub enum SourceCommand {
    /// Fetch upstream's newest version of a `digest` source: the next update of each target that
    /// serves the version of now reads it.
    Refresh { id: String },
}

/// What `source refresh` did.
#[derive(Debug, Serialize, JsonSchema)]
pub struct Refreshed {
    pub source: String,
    /// The versions that the targets served: a target that serves one of them, or none, moves.
    pub from: Vec<String>,
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
    println!(
        "{} {}: the next update of a target that serves it moves to {}",
        refreshed.source,
        refreshed.from.join(", "),
        refreshed.version
    );
    Ok(())
}

/// Fetch the newest version of the `digest` source `id`, and record it with the versions that the
/// targets serve.
pub(super) fn refresh(root: &Path, store: &Store, http: &Http, id: &str) -> Result<Refreshed, Error> {
    let registry = registry(root)?;
    let source = find(&registry, id)?;
    if source.version != VersionScheme::Digest || source.start.is_some() || !fetch::params(source).is_empty() {
        return Err(Code::Usage
            .error(format!("`source refresh` moves a `digest` source without a `start` or params, and `{id}` is not one"))
            .fix("A `date` or `release` source moves when a newer version is due or in the store; a `commit` source, or a source with a `start`, moves with its `start` in data/sources.toml."));
    }
    // A target that cannot be read keeps its version: a refresh never moves what it did not see.
    let serving = releases(root, store)?.into_iter().filter_map(|(_, releases)| releases.ok()?.into_iter().next());
    let from: BTreeSet<String> =
        serving.filter_map(|release| release.versions().remove(&(id.to_string(), Vec::new()))).collect();
    let request = Request { refresh: true, source, version: None, params: Vec::new() };
    let snapshot = fetched(source, fetch::fetch(root, store, http, &request))?;
    let from: Vec<String> = from.into_iter().collect();
    store.put_refreshed(&RefreshedVersion { source: id.into(), from: from.clone(), to: snapshot.version.clone() })?;
    let bytes = snapshot.files.iter().map(|file| file.size).sum();
    Ok(Refreshed { source: id.into(), from, version: snapshot.version, bytes })
}
