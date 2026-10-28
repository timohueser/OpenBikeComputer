//! The archive in `OBC_DATA_ARCHIVE`: whole datasets on a drive of their own. Each file is at
//! `mirror/<host>/<path>` of its `https://` URL. A JSON manifest under `manifest/` lists each file
//! by that path with its size in `bytes` and, when known, its `sha256`. The archive serves only a
//! listed file of the listed size: an unlisted file can be one that is still downloading.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

use super::http::{self, Downloaded, Expect};
use crate::date;
use crate::store::{self, Store};

pub struct Archive {
    root: PathBuf,
    /// Read from the manifests at the first lookup.
    listing: OnceLock<Result<Listing, String>>,
}

#[derive(Default)]
struct Listing {
    /// Each listed file by its path under the root.
    files: HashMap<String, Listed>,
    /// The path of the zip of each prepackaged CLMS file, by dataset UID and file id.
    clms: HashMap<(String, String), String>,
}

#[derive(Deserialize)]
struct Manifest {
    files: Vec<Listed>,
    clms_dataset_uid: Option<String>,
    clms_file_id: Option<String>,
}

#[derive(Deserialize)]
struct Listed {
    path: String,
    bytes: u64,
    sha256: Option<String>,
}

/// A listed file that the archive holds.
pub struct Held {
    /// The path under the root, as the manifest lists it.
    pub path: String,
    pub file: PathBuf,
    pub sha256: Option<String>,
    /// The day the file was last modified, `YYYY-MM-DD`.
    pub day: String,
}

impl Archive {
    pub fn new(root: PathBuf) -> Self {
        Self { root, listing: OnceLock::new() }
    }

    /// The archive in `OBC_DATA_ARCHIVE`, when it is set and not empty.
    pub fn from_env() -> Option<Self> {
        std::env::var_os("OBC_DATA_ARCHIVE").filter(|root| !root.is_empty()).map(|root| Self::new(root.into()))
    }

    /// The file of an `https://` URL without a query, a fragment or a `.` or `..` segment.
    pub fn find(&self, url: &str) -> Result<Option<Held>, String> {
        let Some(rest) = url.strip_prefix("https://").filter(|rest| !rest.contains(['?', '#'])) else {
            return Ok(None);
        };
        if rest.split('/').any(|segment| segment.is_empty() || segment == "." || segment == "..") {
            return Ok(None);
        }
        self.held(&format!("mirror/{rest}"))
    }

    /// The zip of the prepackaged `file` of the CLMS `dataset`: a manifest names both with
    /// `clms_dataset_uid` and `clms_file_id`, and lists the zip as its one file.
    pub fn clms(&self, dataset: &str, file: &str) -> Result<Option<Held>, String> {
        match self.listing()?.clms.get(&(dataset.to_string(), file.to_string())) {
            Some(path) => self.held(path),
            None => Ok(None),
        }
    }

    fn listing(&self) -> Result<&Listing, String> {
        self.listing.get_or_init(|| read_manifests(&self.root.join("manifest"))).as_ref().map_err(Clone::clone)
    }

    fn held(&self, path: &str) -> Result<Option<Held>, String> {
        let Some(listed) = self.listing()?.files.get(path) else { return Ok(None) };
        let file = self.root.join(path);
        let Some(metadata) = fs::metadata(&file).ok().filter(|metadata| metadata.is_file()) else { return Ok(None) };
        if metadata.len() != listed.bytes {
            return Err(format!(
                "{}: {} bytes, but its manifest lists {}",
                file.display(),
                metadata.len(),
                listed.bytes
            ));
        }
        let modified = metadata.modified().map_err(|e| format!("{}: {e}", file.display()))?;
        let seconds = modified.duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.as_secs());
        let day = date::format((seconds / 86_400) as i64);
        Ok(Some(Held { path: path.to_string(), file, sha256: listed.sha256.clone(), day }))
    }
}

impl Held {
    /// Put the file into the store, as a download of `url` that meets `expect` would. `None` when it
    /// is another version: other bytes than `expect.sha256`, or, without that, modified after
    /// `expect.modified_by`. Other bytes than the manifest's are an error.
    pub fn import(&self, store: &Store, url: &str, expect: &Expect) -> Result<Option<Downloaded>, String> {
        if expect.sha256.is_none() && expect.modified_by.is_some_and(|by| self.day.as_str() > by) {
            return Ok(None);
        }
        let part = store.partial(&format!("{}.archive", http::key(url)));
        let parent = part.parent().ok_or("the partial path has no parent")?;
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let _ = fs::remove_file(&part);
        // A hard link costs no space when the store and the archive share a file system.
        if fs::hard_link(&self.file, &part).is_err() {
            let size = fs::metadata(&self.file).map_err(|e| format!("{}: {e}", self.file.display()))?.len();
            store::check_free(&part, size)?;
            fs::copy(&self.file, &part).map_err(|e| format!("{}: {e}", self.file.display()))?;
        }
        let (sha256, size) = store::hash_file(&part)?;
        if let Some(want) = self.sha256.as_deref().filter(|want| *want != sha256) {
            let _ = fs::remove_file(&part);
            return Err(format!("{}: the SHA-256 is {sha256}, but its manifest lists {want}", self.file.display()));
        }
        if expect.sha256.is_some_and(|want| want != sha256) {
            let _ = fs::remove_file(&part);
            return Ok(None);
        }
        let object = store.insert(&part, &sha256)?;
        Ok(Some(Downloaded { object, sha256, size }))
    }
}

/// The files of every `*.json` manifest in `dir`. A manifest that cannot be read fails every
/// lookup, so a bad manifest never passes unseen.
fn read_manifests(dir: &Path) -> Result<Listing, String> {
    let mut listing = Listing::default();
    let Ok(entries) = fs::read_dir(dir) else { return Ok(listing) };
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let manifest: Manifest = serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if let (Some(dataset), Some(file), [zip]) =
            (manifest.clms_dataset_uid, manifest.clms_file_id, manifest.files.as_slice())
        {
            listing.clms.insert((dataset, file), zip.path.clone());
        }
        listing.files.extend(manifest.files.into_iter().map(|file| (file.path.clone(), file)));
    }
    Ok(listing)
}
