//! `obc data archive`: the datasets of `data/archive/` in the roots of `OBC_DATA_ARCHIVE`, see
//! [`crate::datasets`].

use std::path::{Path, PathBuf};

use clap::Subcommand;

use super::{print_json, registry, Code, Error};
use crate::datasets::{self, ArchiveDataset, ArchiveStatus, Dataset};
use crate::engine::runs::bytes;
use crate::fetch::archive;
use crate::store::Store;

#[derive(Subcommand)]
pub enum ArchiveCommand {
    /// Each root with its free space, and each dataset with its root, version, state and files.
    Status,
    /// Get the files of DATASET that its root does not hold, and write its manifest. The same
    /// command resumes it.
    Get {
        dataset: String,
        /// The root of OBC_DATA_ARCHIVE to get it into.
        #[arg(long, value_name = "PATH")]
        root: Option<PathBuf>,
    },
    /// Read each file of the manifest of DATASET and compare its size and SHA-256.
    Verify { dataset: String },
}

pub fn run(root: &Path, command: ArchiveCommand, json: bool) -> Result<(), Error> {
    let registry = registry(root)?;
    let datasets = datasets::load(root, &registry).map_err(|e| Code::InvalidData.error(e))?;
    let roots = archive::roots();
    if !matches!(command, ArchiveCommand::Status) && roots.is_empty() {
        return Err(Code::Usage
            .error("OBC_DATA_ARCHIVE names no root")
            .fix("Set OBC_DATA_ARCHIVE to the archive roots, separated by `:`."));
    }
    let (id, failure) = match command {
        ArchiveCommand::Status => {
            let status = datasets::status(&datasets, &registry, &roots);
            if json {
                return print_json(&status);
            }
            print_status(&status);
            return Ok(());
        }
        ArchiveCommand::Get { dataset: id, root: chosen } => {
            let dataset = defined(&datasets, &id)?;
            let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
            let http = datasets::client(dataset, &home).map_err(|e| Code::Blocked.error(e))?;
            let store = Store::open()?;
            crate::engine::runs::stop_on_signals();
            let got = datasets::get(&http, &store, dataset, &roots, chosen.as_deref())
                .map_err(|e| Code::FetchFailed.error(e))?;
            let failure = got.errors.first().map(|first| {
                Code::FetchFailed
                    .error(format!(
                        "{} of {} files of `{id}` failed; the first, {}: {}",
                        got.errors.len(),
                        got.manifest.files.len() + got.manifest.missing.len(),
                        first.path,
                        first.error
                    ))
                    .fix(format!("Run `obc data archive get {id}` again: it resumes."))
            });
            (id, failure)
        }
        ArchiveCommand::Verify { dataset: id } => {
            defined(&datasets, &id)?;
            let bad = datasets::verify(&Store::open()?, &roots, &id).map_err(|e| Code::Failed.error(e))?;
            let failure = bad.first().map(|first| {
                Code::VerifyFailed
                    .error(format!(
                        "{} files of `{id}` differ from the manifest; the first, {}: {}",
                        bad.len(),
                        first.path,
                        first.error
                    ))
                    .fix(format!("`obc data archive get {id}` downloads them again."))
            });
            (id, failure)
        }
    };
    if let Some(failure) = failure {
        return Err(failure);
    }
    let row = datasets::row(defined(&datasets, &id)?, &roots);
    if json {
        return print_json(&row);
    }
    println!("{}", line(&row));
    Ok(())
}

fn defined<'a>(datasets: &'a [Dataset], id: &str) -> Result<&'a Dataset, Error> {
    datasets
        .iter()
        .find(|dataset| dataset.id == id)
        .ok_or_else(|| Code::Usage.error(format!("no dataset `{id}` in data/archive/")))
}

fn print_status(status: &ArchiveStatus) {
    if status.roots.is_empty() {
        println!("no archive: OBC_DATA_ARCHIVE names no root");
    }
    for root in &status.roots {
        match root.free_bytes {
            Some(free) => println!("root {}: {} free", root.path, bytes(free)),
            None => println!("root {}: does not exist", root.path),
        }
    }
    for row in &status.datasets {
        println!("{}", line(row));
    }
    if !status.unarchived.is_empty() {
        println!("keep = \"archive\" without a dataset: {}", status.unarchived.join(", "));
    }
}

fn line(row: &ArchiveDataset) -> String {
    let state =
        serde_json::to_value(row.state).ok().and_then(|value| value.as_str().map(str::to_string)).unwrap_or_default();
    let mut line = format!(
        "{:<34} {state:<10} {}/{} files, {}, version {}",
        row.dataset,
        row.verified,
        row.listed,
        bytes(row.bytes),
        row.version.as_deref().unwrap_or("-")
    );
    if let Some(root) = &row.root {
        line += &format!(", in {root}");
    }
    if let Some(error) = &row.error {
        line += &format!(": {error}");
    }
    line
}
