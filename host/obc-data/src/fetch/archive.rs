//! The archive in `OBC_DATA_ARCHIVE`: whole datasets on a drive of their own. Each file is at
//! `mirror/<host>/<path>` of its `https://` URL, with the `Last-Modified` of upstream as its
//! modification time. A JSON manifest under `manifest/` lists each file by that path with its size
//! in `bytes` and, when known, its `sha256`. The archive serves only a listed file of the listed
//! size: an unlisted file can be one that is still downloading. A problem of the archive is a
//! warning and no file, so that it never stops a fetch that upstream or the store can serve; only
//! a file whose bytes are not those of its manifest fails the fetch.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use serde::Deserialize;

use super::http::{self, Downloaded, Expect};
use crate::date;
use crate::store::{self, Store};

/// The start of the error of a file whose bytes are not those of its manifest.
const MISMATCH: &str = "archive file ";

/// Whether a fetch error is of an archive file whose bytes are not those of its manifest.
pub fn mismatch(error: &str) -> bool {
    error.starts_with(MISMATCH)
}

pub struct Archive {
    root: PathBuf,
    /// Read from the manifests at the first lookup.
    listing: OnceLock<Listing>,
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
    /// Lowercase hex.
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
    pub fn find(&self, url: &str) -> Option<Held> {
        let rest = url.strip_prefix("https://").filter(|rest| !rest.contains(['?', '#']))?;
        if rest.split('/').any(|segment| segment.is_empty() || segment == "." || segment == "..") {
            return None;
        }
        self.held(&format!("mirror/{rest}"))
    }

    /// The zip of the prepackaged `file` of the CLMS `dataset`: a manifest names both with
    /// `clms_dataset_uid` and `clms_file_id`, and lists the zip as its one file.
    pub fn clms(&self, dataset: &str, file: &str) -> Option<Held> {
        self.held(self.listing().clms.get(&(dataset.to_string(), file.to_string()))?)
    }

    fn listing(&self) -> &Listing {
        self.listing.get_or_init(|| read_manifests(&self.root.join("manifest")))
    }

    fn held(&self, path: &str) -> Option<Held> {
        let listed = self.listing().files.get(path)?;
        let file = self.root.join(path);
        let metadata = fs::metadata(&file).ok().filter(|metadata| metadata.is_file())?;
        if metadata.len() != listed.bytes {
            let (size, bytes) = (metadata.len(), listed.bytes);
            eprintln!(
                "obc data: warning: archive file {} has {size} bytes, but its manifest lists {bytes}",
                file.display()
            );
            return None;
        }
        let seconds = metadata.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        let day = date::format((seconds / 86_400) as i64);
        Some(Held { path: path.to_string(), file, sha256: listed.sha256.clone(), day })
    }
}

impl Held {
    /// Fail when the bytes of the file in place are not those of its manifest.
    pub fn verify(&self) -> Result<(), String> {
        self.check(&store::hash_file(&self.file)?.0)
    }

    fn check(&self, sha256: &str) -> Result<(), String> {
        match self.sha256.as_deref().filter(|want| *want != sha256) {
            Some(want) => Err(format!(
                "{MISMATCH}{}: the SHA-256 is {sha256}, but its manifest lists {want}",
                self.file.display()
            )),
            None => Ok(()),
        }
    }

    /// Copy the file into the store, as a download of `url` that meets `expect` would. `None` when
    /// it is another version: other bytes than `expect.sha256`, or, without that, modified after
    /// `expect.modified_by`. A copy, never a link, so that the store and the archive stay apart.
    pub fn import(&self, store: &Store, url: &str, expect: &Expect) -> Result<Option<Downloaded>, String> {
        if let Some(by) = expect.modified_by.filter(|by| expect.sha256.is_none() && self.day.as_str() > *by) {
            eprintln!("obc data: archive file {} is newer than version {by}: {}", self.file.display(), self.day);
            return Ok(None);
        }
        let part = store.partial(&format!("{}.archive", http::key(url)));
        let parent = part.parent().ok_or("the partial path has no parent")?;
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let size = match fs::metadata(&self.file) {
            Ok(metadata) => metadata.len(),
            Err(error) => {
                eprintln!("obc data: warning: archive file {}: {error}", self.file.display());
                return Ok(None);
            }
        };
        store::check_free(&part, size)?;
        // A part left by an older build can be a link of the archive file: a copy onto it would
        // empty the archive file.
        let _ = fs::remove_file(&part);
        if let Err(error) = fs::copy(&self.file, &part) {
            eprintln!("obc data: warning: archive file {}: {error}", self.file.display());
            let _ = fs::remove_file(&part);
            return Ok(None);
        }
        let (sha256, size) = store::hash_file(&part)?;
        if let Err(error) = self.check(&sha256) {
            let _ = fs::remove_file(&part);
            return Err(error);
        }
        if let Some(want) = expect.sha256.filter(|want| *want != sha256) {
            eprintln!("obc data: archive file {} is not the version with the SHA-256 {want}", self.file.display());
            let _ = fs::remove_file(&part);
            return Ok(None);
        }
        let object = store.insert(&part, &sha256)?;
        Ok(Some(Downloaded { object, sha256, size }))
    }
}

/// The files of every `*.json` manifest in `dir`. A manifest that cannot be read is a warning, and
/// none of its files serves.
fn read_manifests(dir: &Path) -> Listing {
    let mut listing = Listing::default();
    let Ok(entries) = fs::read_dir(dir) else { return listing };
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let read = fs::read_to_string(&path).map_err(|e| e.to_string());
        let manifest = match read.and_then(|text| serde_json::from_str::<Manifest>(&text).map_err(|e| e.to_string())) {
            Ok(manifest) => manifest,
            Err(error) => {
                eprintln!("obc data: warning: archive manifest {}: {error}", path.display());
                continue;
            }
        };
        let (mut files, listed) = (Vec::new(), manifest.files.len());
        for mut file in manifest.files {
            if !confined(&file.path) {
                eprintln!(
                    "obc data: warning: archive manifest {}: `{}` is not under the archive",
                    path.display(),
                    file.path
                );
                continue;
            }
            file.sha256 = file.sha256.map(|sha256| sha256.to_ascii_lowercase());
            files.push(file);
        }
        if let (Some(dataset), Some(file), [zip], 1) =
            (manifest.clms_dataset_uid, manifest.clms_file_id, &files[..], listed)
        {
            listing.clms.insert((dataset, file), zip.path.clone());
        }
        listing.files.extend(files.into_iter().map(|file| (file.path.clone(), file)));
    }
    listing
}

/// Whether `path` is relative and has no empty, `.` or `..` segment, so that it stays under the root.
fn confined(path: &str) -> bool {
    !path.starts_with('/') && path.split('/').all(|segment| !matches!(segment, "" | "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_https_url_finds_its_file() {
        let scratch = crate::store::tests::Scratch::new("archive-find");
        let root = scratch.0.join("archive");
        std::fs::create_dir_all(root.join("mirror/example.org")).unwrap();
        std::fs::create_dir_all(root.join("manifest")).unwrap();
        std::fs::write(root.join("mirror/example.org/x"), b"x").unwrap();
        std::fs::write(scratch.0.join("outside.zip"), b"z").unwrap();
        let outside = r#"{"path": "../outside.zip", "bytes": 1}"#;
        let manifest =
            format!(r#"{{"files": [{outside}, {{"path": "mirror/example.org/x", "bytes": 1, "sha256": "AB"}}]}}"#);
        std::fs::write(root.join("manifest/x.json"), manifest).unwrap();
        let clms = format!(r#"{{"clms_dataset_uid": "d", "clms_file_id": "f", "files": [{outside}]}}"#);
        std::fs::write(root.join("manifest/clms.json"), clms).unwrap();
        let archive = Archive::new(root);
        assert_eq!(archive.find("https://example.org/x").unwrap().sha256.as_deref(), Some("ab"));
        for url in ["http://example.org/x", "https://example.org/x?a=1", "https://example.org/y/../x"] {
            assert!(archive.find(url).is_none(), "{url}");
        }
        assert!(archive.clms("d", "f").is_none(), "a manifest path outside the archive never serves");
    }
}
