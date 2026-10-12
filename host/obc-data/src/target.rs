//! Targets: the places that serve data. `data/targets.toml` says where each target is; what a
//! target holds is in its release (see [`crate::release`]). A key is a path inside the target,
//! with `/` between its parts.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use serde::Deserialize;

use crate::fetch::http::{self, Http};
use crate::r2;
use crate::store::{durable, sha256_hex, Lock, Store};

/// Where a target is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// A folder on this machine. No other machine sees it.
    Dir(PathBuf),
    /// An R2 bucket, or a folder of one: read through the HTTPS origin that serves its keys, and
    /// written with the credentials of `OBC_R2_*`.
    R2(r2::Remote),
}

/// A version of the bytes at a key: a write on that condition fails when the key changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag(pub String);

/// A file of a dir target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub key: String,
    pub size: u64,
}

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
    R2 { public: String, bucket: String, prefix: Option<String> },
    Dir { dir: Option<String> },
}

/// The file of a dir target that one conditional write at a time holds.
const LOCK: &str = ".lock";

/// A number for each part file of this process.
static NEXT: AtomicU64 = AtomicU64::new(0);

/// The targets of `data/targets.toml` in the repository at `root`, by name. A dir target without
/// `dir` is the folder of its name beside the store: a link from one to the other needs one file
/// system.
pub fn load(root: &Path, store: &Store) -> Result<BTreeMap<String, Target>, String> {
    let path = root.join("data/targets.toml");
    let text = fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let store = std::path::absolute(store.root()).map_err(|e| format!("{}: {e}", store.root().display()))?;
    parse(&text, store.parent()).map_err(|e| format!("data/targets.toml: {e}"))
}

fn parse(text: &str, beside: Option<&Path>) -> Result<BTreeMap<String, Target>, String> {
    let entries: BTreeMap<String, Entry> = toml::from_str(text).map_err(|e| e.to_string())?;
    let mut targets = BTreeMap::new();
    for (name, entry) in entries {
        if !crate::is_kebab(&name) {
            return Err(format!("`{name}` is not a target name: lowercase kebab-case"));
        }
        let target = match entry {
            Entry::R2 { public, bucket, prefix } => {
                let host = public.strip_prefix("https://").unwrap_or_default();
                if host.is_empty() || host.contains(['/', '?', '#', '@']) {
                    return Err(format!("target `{name}`: `public` is an https:// origin without a path"));
                }
                if bucket.is_empty()
                    || !bucket.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                {
                    return Err(format!("target `{name}`: `bucket` is the name of an R2 bucket"));
                }
                // A collection deletes keys of the target: a folder outside `test/` can hold live keys.
                let prefix = prefix.unwrap_or_default();
                if !prefix.is_empty() && (!prefix.starts_with("test/") || crate::r2::check_key(&prefix).is_err()) {
                    return Err(format!("target `{name}`: `prefix` is a folder under `test/`"));
                }
                Target::R2(r2::Remote { public, bucket, prefix })
            }
            Entry::Dir { dir } => {
                let dir = match dir {
                    Some(dir) => crate::sources::expand_home(&dir),
                    None => beside.ok_or("the store has no parent folder for a target")?.join(&name),
                };
                if !dir.is_absolute() {
                    return Err(format!("target `{name}`: `dir` is an absolute path, or starts with `~/`"));
                }
                Target::Dir(dir)
            }
        };
        targets.insert(name, target);
    }
    let dirs: Vec<(&String, PathBuf)> = targets
        .iter()
        .filter_map(|(name, target)| match target {
            Target::Dir(dir) => Some(real(dir).map(|dir| (name, dir))),
            Target::R2(_) => None,
        })
        .collect::<Result<_, _>>()?;
    for (a, outer) in &dirs {
        // A collection of the outer target would delete the files of the inner one.
        if let Some((b, _)) = dirs.iter().find(|(b, inner)| b != a && inner.starts_with(outer)) {
            return Err(format!("the folder of target `{b}` is in the folder of target `{a}`"));
        }
    }
    Ok(targets)
}

/// The target `name` of the repository at `root`. A dir target and the store are apart: a clean of
/// the store never deletes what a target serves.
pub fn find(root: &Path, name: &str, store: &Store) -> Result<Target, String> {
    let target = load(root, store)?.remove(name);
    let target = target.ok_or_else(|| format!("data/targets.toml has no target `{name}`"))?;
    if let Target::Dir(dir) = &target {
        let (dir, store) = (real(dir)?, real(store.root())?);
        if dir.starts_with(&store) || store.starts_with(&dir) {
            return Err(format!("target `{name}` ({}) and the store ({}) overlap", dir.display(), store.display()));
        }
    }
    Ok(target)
}

/// `path` with its links resolved, also when its last folders do not exist yet.
fn real(path: &Path) -> Result<PathBuf, String> {
    let path = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let existing = path.ancestors().find(|ancestor| ancestor.exists()).ok_or("no folder of the path exists")?;
    let resolved = existing.canonicalize().map_err(|e| format!("{}: {e}", existing.display()))?;
    Ok(resolved.join(path.strip_prefix(existing).expect("an ancestor is a prefix")))
}

impl Target {
    /// The bytes at `key`, or `None` when the target has no such key. An R2 target is read through
    /// the S3 API when this machine has the credentials, because the public origin can serve a
    /// cached pointer.
    pub fn get(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        if let Target::R2(remote) = self {
            if let Ok(writer) = remote.writer() {
                return Ok(writer.get(key)?.map(|(bytes, _)| bytes));
            }
        }
        self.get_public(key)
    }

    /// The bytes at `key` as a client gets them, or `None`: an R2 target through its public origin,
    /// without credentials.
    pub fn get_public(&self, key: &str) -> Result<Option<Vec<u8>>, String> {
        crate::r2::check_key(key)?;
        match self {
            Target::Dir(dir) => read(&dir.join(key)),
            Target::R2(remote) => match http().bytes(&remote.url(key), "*/*") {
                Ok(bytes) => Ok(Some(bytes)),
                Err(e) if http::not_found(&e) => Ok(None),
                Err(e) => Err(e),
            },
        }
    }

    /// The keys of `files` that the target lacks. A key that holds another size is refused: a key
    /// of a release is immutable.
    pub fn missing(&self, files: &[(String, u64)]) -> Result<Vec<String>, String> {
        let dir = match self {
            Target::Dir(dir) => dir,
            Target::R2(remote) => return r2_missing(remote, files),
        };
        let mut missing = Vec::new();
        for (key, size) in files {
            crate::r2::check_key(key)?;
            match fs::metadata(dir.join(key)) {
                Ok(found) if found.is_dir() => return Err(format!("{key} is a folder of the target, not a file")),
                Ok(found) if found.len() == *size => {}
                Ok(found) => {
                    return Err(format!(
                        "{key} holds {} bytes, not {size}: a write of it broke, and a key is immutable. Fix: \
                         delete {key} from the target, then run the command again",
                        found.len()
                    ))
                }
                Err(e) if e.kind() == ErrorKind::NotFound => missing.push(key.clone()),
                Err(e) => return Err(format!("{key}: {e}")),
            }
        }
        Ok(missing)
    }

    /// The bytes at `key` and their tag, or `None` when the target has no such key.
    pub fn get_tagged(&self, key: &str) -> Result<Option<(Vec<u8>, Tag)>, String> {
        crate::r2::check_key(key)?;
        let dir = match self {
            Target::Dir(dir) => dir,
            Target::R2(remote) => return remote.writer()?.get(key),
        };
        Ok(read(&dir.join(key))?.map(|bytes| {
            let tag = Tag(sha256_hex(&bytes));
            (bytes, tag)
        }))
    }

    /// Write `bytes` at `key` when `condition` holds, and give their tag. `None` when it does not
    /// hold: nothing changed.
    pub fn put(&self, key: &str, bytes: &[u8], condition: &Condition) -> Result<Option<Tag>, String> {
        crate::r2::check_key(key)?;
        let dir = match self {
            Target::Dir(dir) => dir,
            Target::R2(remote) => return remote.writer()?.put(key, bytes, condition),
        };
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
        durable(&path, bytes)?;
        Ok(Some(Tag(sha256_hex(bytes))))
    }

    /// Give each key of `files` the bytes of its file, unless the key has bytes. The key is a link
    /// to the file when both are on one file system, so nothing ever writes a key in place.
    pub fn upload(&self, files: &[(PathBuf, String)]) -> Result<(), String> {
        let dir = match self {
            Target::Dir(dir) => dir,
            Target::R2(remote) => {
                let upload = r2::Upload { cache_control: Some(r2::s3::IMMUTABLE), ..Default::default() };
                return remote.writer()?.bucket()?.put_many(files, &upload);
            }
        };
        for (file, key) in files {
            crate::r2::check_key(key)?;
            let path = dir.join(key);
            let parent = path.parent().expect("a key has a parent folder");
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            match link(file, &path) {
                Err(e) if e.kind() != ErrorKind::AlreadyExists => return Err(format!("{}: {e}", path.display())),
                _ => {}
            }
        }
        Ok(())
    }
}

/// Give `path`, which must not exist, the bytes of `file`: a hard link, or on another file system
/// a copy under a [`part`] name that is then linked. Nothing opens an existing file for writing:
/// it can be a link to an object of the store.
pub(crate) fn link(file: &Path, path: &Path) -> std::io::Result<()> {
    match fs::hard_link(file, path) {
        Err(e) if e.kind() == ErrorKind::CrossesDevices => {
            let part = part(path);
            let _ = fs::remove_file(&part);
            let copied = OpenOptions::new().write(true).create_new(true).open(&part).and_then(|mut part| {
                std::io::copy(&mut File::open(file)?, &mut part)?;
                part.sync_all()
            });
            let linked = copied.and_then(|()| fs::hard_link(&part, path));
            let _ = fs::remove_file(&part);
            linked
        }
        linked => linked,
    }
}

/// A temporary name beside `path` that no other write of this process uses.
pub(crate) fn part(path: &Path) -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    path.with_file_name(format!(".{}.{}.part", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)))
}

impl Target {
    /// Give `store` the object `sha256` from the bytes at `key`. `None` when the target has no such
    /// key; an error when its bytes have another SHA-256.
    pub fn download(&self, key: &str, sha256: &str, store: &Store) -> Result<Option<PathBuf>, String> {
        crate::r2::check_key(key)?;
        let dir = match self {
            Target::Dir(dir) => dir,
            Target::R2(remote) => {
                let url = remote.url(key);
                let _lock = Http::lock(store, &url)?;
                return match http().download(store, &url, &http::Expect { sha256: Some(sha256), ..Default::default() })
                {
                    Ok(downloaded) => Ok(Some(downloaded.object)),
                    Err(e) if http::not_found(&e) => Ok(None),
                    Err(e) => Err(e),
                };
            }
        };
        // Two processes on one store download into other files.
        let part =
            store.partial(&format!("{sha256}.{}.{}.target", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let parent = part.parent().expect("a partial file has a folder");
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        let _ = fs::remove_file(&part);
        // A key is often a link to an object of a store on the same file system.
        match fs::hard_link(dir.join(key), &part).or_else(|_| fs::copy(dir.join(key), &part).map(|_| ())) {
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("{key}: {e}")),
            Ok(()) => {}
        }
        let (held, _) = crate::store::hash_file(&part)?;
        if held != sha256 {
            let _ = fs::remove_file(&part);
            return Err(format!("{key} holds bytes with the SHA-256 {held}, not {sha256}"));
        }
        store.insert(&part, sha256).map(Some)
    }

    /// Every file of a dir target, also a hidden one, sorted by key. Of an R2 target, the keys under
    /// `objects/`, `releases/` and `sources/`: a collection deletes no other key there.
    pub fn list(&self) -> Result<Vec<Listed>, String> {
        let dir = match self {
            Target::Dir(dir) => dir,
            Target::R2(remote) => return r2_list(remote),
        };
        let mut listed = Vec::new();
        let mut pending = vec![dir.clone()];
        while let Some(folder) = pending.pop() {
            let entries = match fs::read_dir(&folder) {
                Ok(entries) => entries,
                Err(e) if e.kind() == ErrorKind::NotFound => continue,
                Err(e) => return Err(format!("{}: {e}", folder.display())),
            };
            for entry in entries {
                let path = entry.map_err(|e| format!("{}: {e}", folder.display()))?.path();
                let found = fs::symlink_metadata(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                if found.is_dir() {
                    pending.push(path);
                    continue;
                }
                let key = path.strip_prefix(dir).expect("below the target").to_string_lossy().replace('\\', "/");
                listed.push(Listed { key, size: found.len() });
            }
        }
        listed.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(listed)
    }

    /// Delete `key` of a dir target, and the folders that it leaves empty below its first part. A key
    /// that the target lacks is no error.
    pub fn remove(&self, key: &str) -> Result<(), String> {
        crate::r2::check_key(key)?;
        let dir = match self {
            Target::Dir(dir) => dir,
            Target::R2(remote) => return remote.writer()?.delete(key),
        };
        let path = dir.join(key);
        match fs::remove_file(&path) {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(format!("{key}: {e}")),
            _ => {}
        }
        let top = dir.join(key.split('/').next().expect("a key has a first part"));
        for folder in path.ancestors().skip(1).take_while(|folder| folder.starts_with(&top) && *folder != top) {
            if fs::remove_dir(folder).is_err() {
                break;
            }
        }
        Ok(())
    }
}

impl Target {
    /// The folder `folder` of the target, as a target of its own.
    pub fn within(&self, folder: &str) -> Target {
        match self {
            Target::Dir(dir) => Target::Dir(dir.join(folder)),
            Target::R2(remote) => Target::R2(r2::Remote { prefix: remote.key(folder), ..remote.clone() }),
        }
    }

    /// The time of now by the clock of the target: of this machine for a dir target, of the
    /// server for an R2 target.
    pub fn now(&self) -> Result<u64, String> {
        match self {
            Target::Dir(_) => Ok(crate::date::now()),
            Target::R2(remote) => Ok(remote.writer()?.head(crate::release::SERVING)?.1),
        }
    }
}

/// The keys of `files` that the R2 target `remote` lacks; a key with another size is refused.
fn r2_missing(remote: &r2::Remote, files: &[(String, u64)]) -> Result<Vec<String>, String> {
    let keys: Vec<String> = files.iter().map(|(key, _)| key.clone()).collect();
    let held = remote.writer()?.bucket()?.stat(&keys)?;
    let mut missing = Vec::new();
    for (key, size) in files {
        match held.get(key) {
            None => missing.push(key.clone()),
            Some(object) if object.bytes == *size => {}
            Some(object) => {
                return Err(format!("{key} holds {} bytes, not {size}, and a key is immutable", object.bytes))
            }
        }
    }
    Ok(missing)
}

/// The keys of the R2 target `remote` under `objects/`, `releases/` and `sources/`, sorted.
fn r2_list(remote: &r2::Remote) -> Result<Vec<Listed>, String> {
    let bucket = remote.writer()?.bucket()?;
    let mut listed = Vec::new();
    for folder in ["objects", "releases", "sources"] {
        let objects = bucket.list(folder)?;
        listed.extend(objects.into_iter().map(|object| Listed { key: object.key, size: object.bytes }));
    }
    listed.sort_by(|a, b| a.key.cmp(&b.key));
    Ok(listed)
}

/// The client of the reads of an R2 target: one for the process, so its connections stay open.
fn http() -> &'static Http {
    static HTTP: OnceLock<Http> = OnceLock::new();
    HTTP.get_or_init(Http::new)
}

fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Hold the lock of the dir target `dir` until the guard drops.
fn lock(dir: &Path) -> Result<Lock, String> {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let path = dir.join(LOCK);
    let file = OpenOptions::new().create(true).truncate(false).write(true).open(&path);
    let file = file.map_err(|e| format!("{}: {e}", path.display()))?;
    file.lock().map_err(|e| format!("lock {}: {e}", path.display()))?;
    Ok(Lock(file))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::tests::Scratch;

    #[test]
    fn the_committed_targets_parse_and_a_wrong_entry_is_refused() {
        let beside = Path::new("/stores");
        let targets = parse(include_str!("../../../data/targets.toml"), Some(beside)).unwrap();
        let prod = r2::Remote {
            public: "https://maps.openbikecomputer.com".into(),
            bucket: "obc-maps".into(),
            prefix: String::new(),
        };
        assert_eq!(targets["prod"], Target::R2(prod));
        assert_eq!(targets["local"], Target::Dir(beside.join("local")), "beside the store");
        let r2 = "kind = \"r2\"\npublic = \"https://maps.example\"\nbucket = \"maps\"\n";
        let folder = parse(&format!("[t]\n{r2}prefix = \"test/run-1\"\n"), None);
        let test =
            r2::Remote { public: "https://maps.example".into(), bucket: "maps".into(), prefix: "test/run-1".into() };
        assert_eq!(folder.unwrap()["t"], Target::R2(test));

        for (text, error) in [
            ("[a]\nkind = \"r2\"\npublic = \"http://maps.example\"\nbucket = \"maps\"\n".into(), "https://"),
            ("[a]\nkind = \"r2\"\npublic = \"https://maps.example/x\"\nbucket = \"maps\"\n".into(), "without a path"),
            ("[a]\nkind = \"r2\"\npublic = \"https://maps.example\"\n".into(), "missing field `bucket`"),
            ("[a]\nkind = \"r2\"\npublic = \"https://maps.example\"\nbucket = \"Maps/x\"\n".into(), "`bucket`"),
            (format!("[a]\n{r2}prefix = \"test/\"\n"), "`prefix`"),
            (format!("[a]\n{r2}prefix = \"test/../x\"\n"), "`prefix`"),
            (format!("[a]\n{r2}prefix = \"planner\"\n"), "`prefix` is a folder under `test/`"),
            ("[a]\nkind = \"dir\"\ndir = \"/x\"\nbucket = \"b\"\n".into(), "unknown field"),
            ("[A]\nkind = \"dir\"\ndir = \"/x\"\n".into(), "kebab-case"),
            ("[a]\nkind = \"dir\"\ndir = \"x\"\n".into(), "absolute"),
            (
                "[a]\nkind = \"dir\"\ndir = \"/x\"\n[b]\nkind = \"dir\"\ndir = \"/x/b\"\n".into(),
                "in the folder of target `a`",
            ),
        ] {
            let refused = parse(&text, Some(beside)).unwrap_err();
            assert!(refused.contains(error), "{refused}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_dir_target_and_the_store_never_overlap() {
        let scratch = Scratch::new("target-find");
        let store = Store::at(scratch.0.join("store"));
        fs::create_dir_all(store.root()).unwrap();
        let through = scratch.0.join("through");
        std::os::unix::fs::symlink(store.root(), &through).unwrap();
        let inside = format!("[inside]\nkind = \"dir\"\ndir = \"{}\"\n", through.join("x/y").display());
        let targets = format!("{inside}[local]\nkind = \"dir\"\n");
        crate::engine::tests::write(&scratch.0.join("data/targets.toml"), &targets);
        assert!(find(&scratch.0, "inside", &store).unwrap_err().contains("overlap"), "a link does not hide it");
        assert_eq!(find(&scratch.0, "local", &store).unwrap(), Target::Dir(scratch.0.join("local")));
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
        assert!(target.put("../x", b"", &Condition::Any).is_err());
    }

    #[test]
    fn an_r2_target_gives_the_store_an_object_through_its_public_origin() {
        let scratch = Scratch::new("target-r2");
        let (bucket, key) = (scratch.0.join("bucket"), "sources/osm/2026#1/a b.pbf");
        fs::create_dir_all(bucket.join("sources/osm/2026#1")).unwrap();
        fs::write(bucket.join(key), b"pbf").unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let listener = runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
        let public = format!("http://{}", listener.local_addr().unwrap());
        let router = axum::Router::new().fallback_service(tower_http::services::ServeDir::new(&bucket));
        runtime.spawn(async move { axum::serve(listener, router).await.unwrap() });
        let (target, store) = (
            Target::R2(r2::Remote { public, bucket: "test".into(), prefix: String::new() }),
            Store::at(scratch.0.join("store")),
        );

        let object = target.download(key, &sha256_hex(b"pbf"), &store).unwrap().unwrap();
        assert_eq!(fs::read(object).unwrap(), b"pbf");
        assert!(target.download(key, &"0".repeat(64), &store).unwrap_err().contains("the SHA-256 is"));
        assert_eq!(target.download("objects/none", &"0".repeat(64), &store).unwrap(), None);
        let remote = r2::Remote { public: "https://maps.example".into(), bucket: "test".into(), prefix: String::new() };
        assert_eq!(remote.url("sources/a#b/c?d"), "https://maps.example/sources/a%23b/c%3Fd");
    }

    #[test]
    fn an_upload_links_what_the_target_lacks_and_never_writes_a_key_in_place() {
        let scratch = Scratch::new("target-upload");
        let target = Target::Dir(scratch.0.join("target"));
        let file = |name: &str, bytes: &str| {
            let path = scratch.0.join(name);
            fs::write(&path, bytes).unwrap();
            path
        };
        let (a, b) = (file("a", "a"), file("b", "b"));
        let sizes = [("objects/a".to_string(), 1), ("objects/b".to_string(), 1)];
        assert_eq!(target.missing(&sizes).unwrap(), ["objects/a", "objects/b"]);
        target.upload(&[(a, "objects/a".into()), (b.clone(), "objects/b".into())]).unwrap();
        assert_eq!(target.missing(&sizes).unwrap(), Vec::<String>::new());
        assert!(target.missing(&[("objects/b".into(), 2)]).unwrap_err().contains("Fix: delete objects/b"));
        assert!(target.missing(&[("objects".into(), 0)]).unwrap_err().contains("a folder"));

        target.upload(&[(file("c", "cc"), "objects/b".into())]).unwrap();
        assert_eq!(target.get("objects/b").unwrap().unwrap(), b"b", "a key keeps its bytes");
        assert_eq!(fs::read(&b).unwrap(), b"b", "and so does the file that it links");
    }
}
