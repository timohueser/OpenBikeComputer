//! The archive in `OBC_DATA_ARCHIVE`: whole datasets on drives of their own. The variable names one
//! or more roots, each a complete archive. A file of an `https://` URL is at `mirror/<host>/<path>`
//! of its root. The manifest `manifest/<dataset>.json` lists each file of a dataset by that path
//! with its size in `bytes` and its `sha256`, and gives the `version` of the dataset. The archive
//! serves only a listed file of the listed size: an unlisted file can be one that is still
//! downloading. A problem of the archive is a warning and no file, so that it never stops a fetch
//! that upstream or the store can serve; only a file whose bytes are not those of its manifest
//! fails the fetch. `crate::datasets` writes the archive.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use super::http::{self, Downloaded, Expect};
use crate::date;
use crate::store::{self, Store};

/// The start of the error of a file whose bytes are not those of its manifest.
const MISMATCH: &str = "archive file ";

/// Whether a fetch error is of an archive file whose bytes are not those of its manifest.
pub fn mismatch(error: &str) -> bool {
    error.starts_with(MISMATCH)
}

/// The roots of `OBC_DATA_ARCHIVE`, in order; none when it is unset or empty.
pub fn roots() -> Vec<PathBuf> {
    std::env::var_os("OBC_DATA_ARCHIVE")
        .map(|value| std::env::split_paths(&value).filter(|root| !root.as_os_str().is_empty()).collect())
        .unwrap_or_default()
}

/// `manifest/<dataset>.json`, as `crate::datasets` writes it.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Manifest {
    #[serde(default)]
    pub dataset: String,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub coverage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The day of the last `archive get`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub got: Option<String>,
    pub files: Vec<Listed>,
    #[serde(default)]
    pub missing: Vec<Missing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clms_dataset_uid: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clms_file_id: Option<String>,
}

/// A verified file of a manifest.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct Listed {
    /// The path under the root.
    pub path: String,
    pub bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// The upstream `Last-Modified` day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<String>,
    /// The upstream `ETag`, with its quotes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,
}

/// A listed file that the root does not hold.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, schemars::JsonSchema)]
pub struct Missing {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bytes: Option<u64>,
    /// The SHA-256 that the manifest recorded for a kept file: a kept file has no other source, so
    /// only those bytes take it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    pub error: String,
}

impl Manifest {
    pub fn read(path: &Path) -> Result<Self, String> {
        let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn write(&self, path: &Path) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        store::write_atomic(path, format!("{text}\n").as_bytes())
    }
}

pub struct Archive {
    roots: Vec<PathBuf>,
    /// Read from the manifests at the first lookup.
    listing: OnceLock<Listing>,
}

#[derive(Default)]
struct Listing {
    /// Each listed file by its path under its root.
    files: HashMap<String, Entry>,
    /// The path of the zip of each prepackaged CLMS file, by dataset UID and file id.
    clms: HashMap<(String, String), String>,
}

struct Entry {
    /// The index of its root.
    root: usize,
    bytes: u64,
    sha256: Option<String>,
    day: Option<Arc<str>>,
}

/// A listed file that the archive holds.
pub struct Held {
    /// The path under the root, as the manifest lists it.
    pub path: String,
    pub file: PathBuf,
    /// Lowercase hex.
    pub sha256: Option<String>,
    /// The upstream `Last-Modified` day of the file, or else the version of its dataset when that
    /// is a day.
    pub day: Option<String>,
}

impl Archive {
    pub fn new(roots: Vec<PathBuf>) -> Self {
        Self { roots, listing: OnceLock::new() }
    }

    /// The archive of `OBC_DATA_ARCHIVE`, when it names a root.
    pub fn from_env() -> Option<Self> {
        Some(roots()).filter(|roots| !roots.is_empty()).map(Self::new)
    }

    /// The file of an `https://` URL without a query, a fragment or a `.` or `..` segment.
    pub fn find(&self, url: &str) -> Option<Held> {
        self.held(&mirror_path(url)?)
    }

    /// The zip of the prepackaged `file` of the CLMS `dataset`: a manifest names both with
    /// `clms_dataset_uid` and `clms_file_id`, and lists the zip as its one file.
    pub fn clms(&self, dataset: &str, file: &str) -> Option<Held> {
        self.held(self.listing().clms.get(&(dataset.to_string(), file.to_string()))?)
    }

    fn listing(&self) -> &Listing {
        self.listing.get_or_init(|| {
            let mut listing = Listing::default();
            for (root, dir) in self.roots.iter().enumerate() {
                if !dir.is_dir() {
                    eprintln!("obc data: warning: the archive root {} does not exist: mount its drive", dir.display());
                }
                read_manifests(&dir.join("manifest"), root, &mut listing);
            }
            listing
        })
    }

    fn held(&self, path: &str) -> Option<Held> {
        let entry = self.listing().files.get(path)?;
        let file = self.roots[entry.root].join(path);
        let metadata = fs::metadata(&file).ok().filter(|metadata| metadata.is_file())?;
        if metadata.len() != entry.bytes {
            let (size, bytes) = (metadata.len(), entry.bytes);
            eprintln!(
                "obc data: warning: archive file {} has {size} bytes, but its manifest lists {bytes}",
                file.display()
            );
            return None;
        }
        Some(Held {
            path: path.to_string(),
            file,
            sha256: entry.sha256.clone(),
            day: entry.day.as_deref().map(str::to_string),
        })
    }
}

/// `mirror/<host>/<path>` of an `https://` URL without a query, a fragment or a `.` or `..` segment.
pub fn mirror_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://").filter(|rest| !rest.contains(['?', '#']))?;
    Some(format!("mirror/{rest}")).filter(|path| confined(path))
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
    /// it is another version: other bytes than `expect.sha256`, or, without that, no day of the
    /// file on or before `expect.modified_by`. A copy, never a link, so that the store and the
    /// archive stay apart.
    pub fn import(&self, store: &Store, url: &str, expect: &Expect) -> Result<Option<Downloaded>, String> {
        let after = |by: &str| self.day.as_deref().is_none_or(|day| day > by);
        if let Some(by) = expect.modified_by.filter(|by| expect.sha256.is_none() && after(by)) {
            eprintln!(
                "obc data: archive file {} changed on {}, after the version {by}",
                self.file.display(),
                self.day.as_deref().unwrap_or("an unknown day")
            );
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

/// Add the files of every `*.json` manifest in `dir` of the root with index `root`. A path that an
/// earlier root lists stays with that root. A manifest that cannot be read or has no `version` is
/// a warning, and none of its files serves.
fn read_manifests(dir: &Path, root: usize, listing: &mut Listing) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for path in entries.flatten().map(|entry| entry.path()) {
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        let manifest = match Manifest::read(&path) {
            Ok(Manifest { version: None, .. }) => Err(format!("{}: no `version`", path.display())),
            read => read,
        };
        let manifest = match manifest {
            Ok(manifest) => manifest,
            Err(error) => {
                eprintln!("obc data: warning: archive manifest {error}");
                continue;
            }
        };
        // A file without its own day takes the version of a date dataset.
        let version: Option<Arc<str>> =
            manifest.version.filter(|version| date::parse(version).is_some()).map(Into::into);
        let (mut files, listed) = (Vec::new(), manifest.files.len());
        for file in manifest.files {
            if !confined(&file.path) {
                eprintln!(
                    "obc data: warning: archive manifest {}: `{}` is not under the archive",
                    path.display(),
                    file.path
                );
                continue;
            }
            files.push(file);
        }
        if let (Some(dataset), Some(file), [zip], 1) =
            (manifest.clms_dataset_uid, manifest.clms_file_id, &files[..], listed)
        {
            listing.clms.entry((dataset, file)).or_insert_with(|| zip.path.clone());
        }
        for file in files {
            let sha256 = file.sha256.map(|sha256| sha256.to_ascii_lowercase());
            let day = file.modified.map(Into::into).or_else(|| version.clone());
            let entry = Entry { root, bytes: file.bytes, sha256, day };
            listing.files.entry(file.path).or_insert(entry);
        }
    }
}

/// Whether `path` is relative and has no empty, `.` or `..` segment, so that it stays under the root.
pub fn confined(path: &str) -> bool {
    !path.starts_with('/') && path.split('/').all(|segment| !matches!(segment, "" | "." | ".."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_plain_https_url_of_a_versioned_manifest_finds_its_file_and_the_first_root_wins() {
        let scratch = crate::store::tests::Scratch::new("archive-find");
        let (first, second) = (scratch.0.join("first"), scratch.0.join("second"));
        for (root, bytes) in [(&first, &b"x"[..]), (&second, &b"yy"[..])] {
            std::fs::create_dir_all(root.join("mirror/example.org")).unwrap();
            std::fs::create_dir_all(root.join("manifest")).unwrap();
            std::fs::write(root.join("mirror/example.org/x"), bytes).unwrap();
        }
        std::fs::write(scratch.0.join("outside.zip"), b"z").unwrap();
        let outside = r#"{"path": "../outside.zip", "bytes": 1}"#;
        let x = r#"{"path": "mirror/example.org/x", "bytes": 1, "sha256": "AB", "modified": "2026-10-05"}"#;
        let manifest = format!(r#"{{"version": "v1", "files": [{outside}, {x}]}}"#);
        std::fs::write(first.join("manifest/x.json"), manifest).unwrap();
        let clms =
            format!(r#"{{"version": "v1", "clms_dataset_uid": "d", "clms_file_id": "f", "files": [{outside}]}}"#);
        std::fs::write(first.join("manifest/clms.json"), clms).unwrap();
        let y = r#"{"path": "mirror/example.org/x", "bytes": 2}, {"path": "mirror/example.org/y", "bytes": 2}"#;
        std::fs::write(second.join("manifest/x.json"), format!(r#"{{"version": "2026-10-02", "files": [{y}]}}"#))
            .unwrap();
        std::fs::write(second.join("mirror/example.org/y"), b"yy").unwrap();
        std::fs::write(
            second.join("manifest/old.json"),
            r#"{"files": [{"path": "mirror/example.org/z", "bytes": 1}]}"#,
        )
        .unwrap();
        std::fs::write(second.join("mirror/example.org/z"), b"z").unwrap();

        let archive = Archive::new(vec![first, second.clone()]);
        let held = archive.find("https://example.org/x").unwrap();
        assert_eq!(
            (held.sha256.as_deref(), held.day.as_deref()),
            (Some("ab"), Some("2026-10-05")),
            "the first root serves"
        );
        let held = archive.find("https://example.org/y").unwrap();
        assert_eq!(
            (held.file, held.day.as_deref()),
            (second.join("mirror/example.org/y"), Some("2026-10-02")),
            "the day of the dataset"
        );
        assert!(archive.find("https://example.org/z").is_none(), "a manifest without `version` never serves");
        for url in ["http://example.org/x", "https://example.org/x?a=1", "https://example.org/y/../x"] {
            assert!(archive.find(url).is_none(), "{url}");
        }
        assert!(archive.clms("d", "f").is_none(), "a manifest path outside the archive never serves");
    }
}
