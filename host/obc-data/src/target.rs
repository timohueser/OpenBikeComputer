//! Targets: the places that serve data. `data/targets.toml` says where each target is; what a
//! target holds is in its release (see [`crate::release`]). A key is a path inside the target,
//! with `/` between its parts.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::fetch::http::{self, Http};
use crate::store::{sha256_hex, write_atomic, Store};

/// Where a target is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A folder on this machine. No other machine sees it.
    Dir(PathBuf),
    /// An R2 bucket, and the HTTPS URL that serves its keys.
    R2 { bucket: String, public: String },
}

/// A version of the bytes at a key: a write on that condition fails when the key changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag(pub String);

/// When a conditional write may replace the bytes at a key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Condition {
    Any,
    /// The key holds nothing.
    Absent,
    /// The key holds the bytes of this tag.
    Match(Tag),
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum Entry {
    R2 { bucket: String, public: String },
    Dir { dir: String },
}

/// The file of a dir target that one conditional write at a time holds.
const LOCK: &str = ".lock";

/// `obc data` reaches an R2 target only through its public URL, which serves reads.
const READ_ONLY: &str = "`obc data` only reads an R2 target, through its public URL";

/// The targets of `data/targets.toml` in the repository at `root`, by name.
pub fn load(root: &Path) -> Result<BTreeMap<String, Target>, String> {
    let path = root.join("data/targets.toml");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("data/targets.toml: {e}"))
}

fn parse(text: &str) -> Result<BTreeMap<String, Target>, String> {
    let entries: BTreeMap<String, Entry> = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut targets = BTreeMap::new();
    for (name, entry) in entries {
        if !crate::is_kebab(&name) {
            return Err(format!("`{name}` is not a target name: lowercase kebab-case"));
        }
        let target = match entry {
            Entry::R2 { bucket, public } => {
                let host = public.strip_prefix("https://").unwrap_or_default();
                if host.is_empty() || host.contains(['/', '?', '#', '@']) {
                    return Err(format!("target `{name}`: `public` is an https:// origin without a path"));
                }
                if bucket.is_empty() {
                    return Err(format!("target `{name}`: `bucket` is empty"));
                }
                Target::R2 { bucket, public }
            }
            Entry::Dir { dir } => {
                let dir = crate::sources::expand_home(&dir);
                if !dir.is_absolute() {
                    return Err(format!("target `{name}`: `dir` is an absolute path, or starts with `~/`"));
                }
                Target::Dir(dir)
            }
        };
        targets.insert(name, target);
    }
    Ok(targets)
}

/// The target `name` of the repository at `root`. A dir target and the store are apart: a clean of
/// the store never deletes what a target serves.
pub fn find(root: &Path, name: &str, store: &Store) -> Result<Target, String> {
    let target = load(root)?.remove(name).ok_or_else(|| format!("data/targets.toml has no target `{name}`"))?;
    if let Target::Dir(dir) = &target {
        let store = store.root();
        if dir.starts_with(store) || store.starts_with(dir) {
            return Err(format!("target `{name}` ({}) and the store ({}) overlap", dir.display(), store.display()));
        }
    }
    Ok(target)
}

impl Target {
    /// The bytes at `key`, or `None` when the target has no such key.
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        crate::r2::check_key(key)?;
        match self {
            Target::Dir(dir) => read(&dir.join(key)),
            Target::R2 { public, .. } => match Http::new().bytes(&format!("{public}/{key}"), "*/*") {
                Ok(bytes) => Ok(Some(bytes)),
                Err(e) if http::not_found(&e) => Ok(None),
                Err(e) => Err(e),
            },
        }
    }

    /// Whether the target has `key`.
    pub fn has(&self, key: &str) -> Result<bool, String> {
        crate::r2::check_key(key)?;
        match self {
            Target::Dir(dir) => Ok(dir.join(key).is_file()),
            Target::R2 { .. } => Err(READ_ONLY.into()),
        }
    }

    /// The bytes at `key` and their tag, or `None` when the target has no such key.
    pub fn get_tagged(&self, key: &str) -> Result<Option<(Vec<u8>, Tag)>, String> {
        crate::r2::check_key(key)?;
        match self {
            Target::Dir(dir) => Ok(read(&dir.join(key))?.map(|bytes| {
                let tag = Tag(sha256_hex(&bytes));
                (bytes, tag)
            })),
            Target::R2 { .. } => Err(READ_ONLY.into()),
        }
    }

    /// Write `bytes` at `key` when `condition` holds, and give their tag. `None` when it does not
    /// hold: nothing changed.
    pub fn put(&self, key: &str, bytes: &[u8], condition: &Condition) -> Result<Option<Tag>, String> {
        crate::r2::check_key(key)?;
        let Target::Dir(dir) = self else { return Err(READ_ONLY.into()) };
        let path = dir.join(key);
        // A dir target is on a local file system: the lock and the rename are atomic there.
        let _lock = lock(dir)?;
        let now = read(&path)?.map(|bytes| Tag(sha256_hex(&bytes)));
        let holds = match condition {
            Condition::Any => true,
            Condition::Absent => now.is_none(),
            Condition::Match(tag) => now.as_ref() == Some(tag),
        };
        if !holds {
            return Ok(None);
        }
        write_atomic(&path, bytes)?;
        Ok(Some(Tag(sha256_hex(bytes))))
    }

    /// Give each key of `files` that the target lacks the bytes of its file, and return those keys.
    /// A key is immutable: one that holds another size is refused, and nothing after it is written.
    pub fn upload(&self, files: &[(PathBuf, String)]) -> Result<Vec<String>, String> {
        let Target::Dir(dir) = self else { return Err(READ_ONLY.into()) };
        let mut uploaded = Vec::new();
        for (file, key) in files {
            crate::r2::check_key(key)?;
            let path = dir.join(key);
            let size =
                |path: &Path| fs::metadata(path).map(|m| m.len()).map_err(|e| format!("{}: {e}", path.display()));
            if path.is_file() {
                if size(&path)? != size(file)? {
                    return Err(format!("{key} holds another size than {}; it is immutable", file.display()));
                }
                continue;
            }
            let parent = path.parent().expect("a key has a parent folder");
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            let name = path.file_name().expect("a key has a name").to_string_lossy();
            let part = parent.join(format!(".{name}.{}.part", std::process::id()));
            // A link costs no space when the target and the store share a file system.
            fs::hard_link(file, &part)
                .or_else(|_| fs::copy(file, &part).map(drop))
                .and_then(|()| fs::rename(&part, &path))
                .map_err(|e| {
                    let _ = fs::remove_file(&part);
                    format!("{}: {e}", path.display())
                })?;
            uploaded.push(key.clone());
        }
        Ok(uploaded)
    }
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Hold the lock of the dir target `dir` until the file drops.
fn lock(dir: &Path) -> Result<File, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(LOCK);
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(&path);
    let file = file.map_err(|e| format!("{}: {e}", path.display()))?;
    file.lock().map_err(|e| format!("lock {}: {e}", path.display()))?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    #[test]
    fn the_committed_targets_parse_and_a_wrong_entry_is_refused() {
        let targets = parse(include_str!("../../../data/targets.toml")).unwrap();
        assert!(matches!(&targets["prod"], Target::R2 { bucket, .. } if bucket == "obc-maps"));
        assert!(matches!(&targets["local"], Target::Dir(dir) if dir.is_absolute()));

        for (text, error) in [
            ("[a]\nkind = \"r2\"\nbucket = \"b\"\npublic = \"http://maps.example\"\n", "https://"),
            ("[a]\nkind = \"r2\"\nbucket = \"b\"\npublic = \"https://maps.example/x\"\n", "without a path"),
            ("[a]\nkind = \"dir\"\ndir = \"/x\"\nbucket = \"b\"\n", "unknown field"),
            ("[A]\nkind = \"dir\"\ndir = \"/x\"\n", "kebab-case"),
            ("[a]\nkind = \"dir\"\ndir = \"x\"\n", "absolute"),
        ] {
            let refused = parse(text).unwrap_err();
            assert!(refused.contains(error), "{refused}");
        }

        let scratch = Scratch::new("target-find");
        let store = Store::at(scratch.0.join("store"));
        let inside = format!("[inside]\nkind = \"dir\"\ndir = \"{}\"\n", scratch.0.join("store/x").display());
        crate::engine::tests::write(&scratch.0.join("data/targets.toml"), &inside);
        assert!(find(&scratch.0, "inside", &store).unwrap_err().contains("overlap"));
        assert!(find(&scratch.0, "other", &store).unwrap_err().contains("no target `other`"));
    }

    #[test]
    fn a_conditional_write_succeeds_once_for_each_tag() {
        let scratch = Scratch::new("target-condition");
        let target = Target::Dir(scratch.0.join("target"));
        let first = target.put("release.json", b"one", &Condition::Absent).unwrap().unwrap();
        assert_eq!(target.put("release.json", b"two", &Condition::Absent).unwrap(), None, "the key holds bytes");
        assert_eq!(target.get_tagged("release.json").unwrap(), Some((b"one".to_vec(), first.clone())));

        let second = target.put("release.json", b"two", &Condition::Match(first.clone())).unwrap().unwrap();
        assert_eq!(target.put("release.json", b"three", &Condition::Match(first)).unwrap(), None, "an old tag");
        assert_eq!(target.get("release.json").unwrap().unwrap(), b"two");
        assert!(target.put("release.json", b"four", &Condition::Match(second)).unwrap().is_some());

        let target = &target;
        let won = std::thread::scope(|scope| {
            let tries: Vec<_> =
                (0..8u8).map(|n| scope.spawn(move || target.put("lock.json", &[n], &Condition::Absent))).collect();
            tries.into_iter().map(|try_| try_.join()).filter(|put| matches!(put, Ok(Ok(Some(_))))).count()
        });
        assert_eq!(won, 1, "one of the writers that start together");
        assert_eq!(target.get_tagged("removed.jsonl").unwrap(), None);
        assert!(target.put("../x", b"", &Condition::Any).is_err());
    }

    #[test]
    fn an_upload_writes_only_what_the_target_lacks_and_never_replaces_a_key() {
        let scratch = Scratch::new("target-upload");
        let target = Target::Dir(scratch.0.join("target"));
        let file = |name: &str, bytes: &str| {
            let path = scratch.0.join(name);
            fs::write(&path, bytes).unwrap();
            path
        };
        let (a, b) = (file("a", "a"), file("b", "b"));
        let files = [(a, "objects/a".to_string()), (b, "objects/b".to_string())];
        assert_eq!(target.upload(&files).unwrap(), ["objects/a", "objects/b"]);
        assert_eq!(target.upload(&files).unwrap(), Vec::<String>::new());
        assert_eq!(target.get("objects/b").unwrap().unwrap(), b"b");
        let other = [(file("c", "cc"), "objects/b".to_string())];
        assert!(target.upload(&other).unwrap_err().contains("another size"));
        assert_eq!(target.get("objects/b").unwrap().unwrap(), b"b");
    }
}
