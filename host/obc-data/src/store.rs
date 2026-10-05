//! The local store: read-only objects named by their SHA-256, the snapshot records that say
//! which objects are which source version, the receipts that say which objects are which layer,
//! and the events of each run. `specs/obc-data.md` describes the layout.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::engine::Receipt;

/// The files of one source version, as fetched.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub source: String,
    pub version: String,
    pub files: Vec<FileRecord>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FileRecord {
    /// The last segment of `url`: the name a step gives the file when it needs one.
    pub name: String,
    pub url: String,
    pub size: u64,
    pub sha256: String,
    /// `YYYY-MM-DDTHH:MM:SSZ`
    pub retrieved: String,
}

impl Snapshot {
    pub fn file(&self, url: &str) -> Option<&FileRecord> {
        self.files.iter().find(|file| file.url == url)
    }
}

pub struct Store {
    root: PathBuf,
}

/// Held while it lives; the operating system releases it when the process ends.
pub struct Lock(#[allow(dead_code)] File);

impl Store {
    /// The store at `OBC_DATA_STORE`, or else at `~/.cache/openbikecomputer/store`.
    pub fn open() -> Result<Self, String> {
        match std::env::var_os("OBC_DATA_STORE").filter(|dir| !dir.is_empty()) {
            Some(dir) => Ok(Self::at(dir)),
            None => {
                let home = std::env::var_os("HOME").ok_or("HOME is not set; set OBC_DATA_STORE")?;
                Ok(Self::at(Path::new(&home).join(".cache/openbikecomputer/store")))
            }
        }
    }

    pub fn at(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn object(&self, sha256: &str) -> PathBuf {
        self.root.join("objects").join(&sha256[..2]).join(sha256)
    }

    /// A file that a download writes and a later download resumes.
    pub fn partial(&self, name: &str) -> PathBuf {
        self.root.join("partial").join(name)
    }

    /// Move `file`, whose SHA-256 is `sha256`, into the objects and make it read-only. `file` must
    /// be in the store: a rename on one file system is atomic, so an object is always whole.
    pub fn insert(&self, file: &Path, sha256: &str) -> Result<PathBuf, String> {
        let object = self.object(sha256);
        if object.is_file() {
            fs::remove_file(file).map_err(|e| format!("{}: {e}", file.display()))?;
            return Ok(object);
        }
        let mut permissions = fs::metadata(file).map_err(|e| format!("{}: {e}", file.display()))?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(file, permissions).map_err(|e| format!("{}: {e}", file.display()))?;
        create_parent(&object)?;
        fs::rename(file, &object).map_err(|e| format!("{}: {e}", object.display()))?;
        Ok(object)
    }

    /// Wait for the lock of `key`, and hold it until the guard drops.
    pub fn lock(&self, key: &str) -> Result<Lock, String> {
        let (file, path) = self.lock_file(key)?;
        file.lock().map_err(|e| format!("lock {}: {e}", path.display()))?;
        Ok(Lock(file))
    }

    /// The lock of `key`, or `None` when another holds it.
    pub fn try_lock(&self, key: &str) -> Result<Option<Lock>, String> {
        let (file, path) = self.lock_file(key)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(Lock(file))),
            Err(TryLockError::WouldBlock) => Ok(None),
            Err(TryLockError::Error(e)) => Err(format!("lock {}: {e}", path.display())),
        }
    }

    fn lock_file(&self, key: &str) -> Result<(File, PathBuf), String> {
        let name: String =
            key.chars().map(|c| if c.is_ascii_alphanumeric() || "@.-".contains(c) { c } else { '_' }).collect();
        let path = self.root.join("locks").join(format!("{name}.lock"));
        create_parent(&path)?;
        let file = OpenOptions::new().create(true).truncate(false).write(true).open(&path);
        Ok((file.map_err(|e| format!("{}: {e}", path.display()))?, path))
    }

    fn snapshot_path(&self, source: &str, version: &str) -> PathBuf {
        self.root.join("snapshots").join(source).join(format!("{version}.json"))
    }

    pub fn snapshot(&self, source: &str, version: &str) -> Result<Option<Snapshot>, String> {
        read_record(&self.snapshot_path(source, version))
    }

    pub fn put_snapshot(&self, snapshot: &Snapshot) -> Result<(), String> {
        write_record(&self.snapshot_path(&snapshot.source, &snapshot.version), snapshot)
    }

    /// Every snapshot of `source` in the store.
    pub fn snapshots(&self, source: &str) -> Result<Vec<Snapshot>, String> {
        read_records(&self.root.join("snapshots").join(source))
    }

    fn layer_path(&self, key: &str) -> PathBuf {
        self.root.join("layers").join(format!("{key}.json"))
    }

    /// The receipt of the layer with this key.
    pub fn layer(&self, key: &str) -> Result<Option<Receipt>, String> {
        read_record(&self.layer_path(key))
    }

    pub fn put_layer(&self, receipt: &Receipt) -> Result<(), String> {
        write_record(&self.layer_path(&receipt.key), receipt)
    }

    /// Every receipt in the store.
    pub fn layers(&self) -> Result<Vec<Receipt>, String> {
        read_records(&self.root.join("layers"))
    }

    fn code_path(&self, hash: &str) -> PathBuf {
        self.root.join("code").join(format!("{hash}.json"))
    }

    /// The code files of a code hash: path to SHA-256.
    pub fn code(&self, hash: &str) -> Result<Option<BTreeMap<String, String>>, String> {
        read_record(&self.code_path(hash))
    }

    pub fn put_code(&self, hash: &str, files: &BTreeMap<String, String>) -> Result<(), String> {
        let path = self.code_path(hash);
        if path.is_file() {
            return Ok(());
        }
        write_record(&path, files)
    }

    /// The events of a run, one JSON object per line.
    pub fn run(&self, id: &str) -> PathBuf {
        self.root.join("runs").join(format!("{id}.jsonl"))
    }
}

fn read_record<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text).map(Some).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The records in `dir`, or none when it does not exist.
fn read_records<T: DeserializeOwned>(dir: &Path) -> Result<Vec<T>, String> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut records = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.extension() == Some("json".as_ref()) {
            records.extend(read_record(&path)?);
        }
    }
    Ok(records)
}

fn write_record(path: &Path, record: &impl Serialize) -> Result<(), String> {
    let text = serde_json::to_string_pretty(record).map_err(|e| e.to_string())?;
    write_atomic(path, text.as_bytes())
}

/// Write `bytes` to a temporary file beside `path`, then rename it: a reader sees the old file or
/// the new one, never a part.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    create_parent(path)?;
    let name = path.file_name().ok_or_else(|| format!("{} has no file name", path.display()))?;
    let temporary = path.with_file_name(format!(".{}.{}.tmp", name.to_string_lossy(), std::process::id()));
    let written = File::create(&temporary).and_then(|mut file| {
        file.write_all(bytes)?;
        file.sync_all()
    });
    written.and_then(|()| fs::rename(&temporary, path)).map_err(|e| {
        let _ = fs::remove_file(&temporary);
        format!("{}: {e}", path.display())
    })
}

/// The SHA-256 of a file, in lowercase hex, and its size.
pub fn hash_file(path: &Path) -> Result<(String, u64), String> {
    let mut file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut size = 0;
    loop {
        let read = file.read(&mut buffer).map_err(|e| format!("{}: {e}", path.display()))?;
        if read == 0 {
            return Ok((hex(&hasher.finalize()), size));
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn create_parent(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| format!("{} has no parent", path.display()))?;
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// An empty directory that is removed when the test ends.
    pub(crate) struct Scratch(pub PathBuf);

    impl Scratch {
        pub(crate) fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("obc-data-{name}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            make_writable(&self.0);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Windows refuses to delete a read-only file.
    fn make_writable(dir: &Path) {
        for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                make_writable(&path);
            } else if let Ok(metadata) = fs::metadata(&path) {
                let mut permissions = metadata.permissions();
                #[allow(clippy::permissions_set_readonly_false)]
                permissions.set_readonly(false);
                let _ = fs::set_permissions(&path, permissions);
            }
        }
    }

    #[test]
    fn an_atomic_write_replaces_the_file_and_leaves_no_temporary() {
        let scratch = Scratch::new("atomic");
        let path = scratch.0.join("deep/record.json");
        write_atomic(&path, b"old").unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"new");
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1);
    }

    #[test]
    fn an_object_is_named_by_its_digest_and_read_only() {
        let scratch = Scratch::new("object");
        let store = Store::at(&scratch.0);
        let mut objects = Vec::new();
        for copy in ["a", "b"] {
            let file = store.partial(copy);
            write_atomic(&file, b"bytes").unwrap();
            let (sha256, size) = hash_file(&file).unwrap();
            assert_eq!((sha256.as_str(), size), (sha256_hex(b"bytes").as_str(), 5));
            objects.push(store.insert(&file, &sha256).unwrap());
            assert!(!file.exists());
        }
        assert_eq!(objects[0], objects[1], "the same bytes are one object");
        assert!(fs::metadata(&objects[0]).unwrap().permissions().readonly());
    }

    #[test]
    fn a_lock_waits_for_the_holder() {
        let scratch = Scratch::new("lock");
        let store = Store::at(&scratch.0);
        let held = store.lock("osm-planet@2026-10-04").unwrap();
        let acquired = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                let _lock = store.lock("osm-planet@2026-10-04").unwrap();
                acquired.store(true, Ordering::SeqCst);
            });
            let _other = store.lock("osm-planet@2026-09-27").unwrap();
            std::thread::sleep(Duration::from_millis(100));
            assert!(!acquired.load(Ordering::SeqCst), "the second holder waits");
            drop(held);
        });
        assert!(acquired.load(Ordering::SeqCst));
    }
}
