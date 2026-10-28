//! The datasets of the archive: their definitions in `data/archive/<dataset>.toml`, and how
//! `obc data archive` lists, gets and verifies them in a root of `OBC_DATA_ARCHIVE`. The fetchers
//! read the archive through [`crate::fetch::archive`].

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::date;
use crate::fetch::archive::{self, Listed, Manifest, Missing};
use crate::fetch::http::{self, Expect, Http, S3Keys};
use crate::sources::{Keep, Registry, VersionScheme};
use crate::store;

/// The CDSE S3 API: its requests are signed with the keys of [`CDSE_KEYS`].
const CDSE: &str = "https://eodata.dataspace.copernicus.eu";
const CDSE_KEYS: &str = "~/.config/openbikecomputer/cdse-s3.env";
/// Downloads at once. Chosen, not measured: the tools before this one ran 4 to 6.
const WORKERS: usize = 4;
/// How often a get writes the manifest while it runs.
const SAVE: Duration = Duration::from_secs(60);
/// The tries of one page of a bucket listing.
const TRIES: u32 = 4;

/// `data/archive/<dataset>.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dataset {
    #[serde(skip)]
    pub id: String,
    pub name: String,
    pub sources: Vec<String>,
    pub coverage: String,
    pub version: Option<String>,
    #[serde(default)]
    pub urls: Vec<String>,
    #[serde(default)]
    pub s3: Vec<S3>,
    #[serde(default)]
    pub kept: bool,
    pub clms: Option<Clms>,
    pub notes: Option<String>,
    /// The `version` kind of its sources.
    #[serde(skip)]
    scheme: Option<VersionScheme>,
}

/// The objects of a bucket listing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct S3 {
    /// The `https://` URL of the bucket: the URL of an object is `<bucket>/<key>`.
    pub bucket: String,
    pub prefix: String,
    /// A pattern for the key after `prefix`, in which `*` stands for characters other than `/`.
    #[serde(rename = "match")]
    pub pattern: Option<String>,
}

/// A prepackaged file of the CLMS download API.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Clms {
    pub dataset: String,
    pub file: String,
}

/// The datasets of `data/archive/` in the repository `root`, each checked against `registry`.
pub fn load(root: &Path, registry: &Registry) -> Result<Vec<Dataset>, String> {
    let dir = root.join("data/archive");
    let entries = fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "toml"))
        .collect();
    paths.sort();
    paths
        .iter()
        .map(|path| {
            let id = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
            let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            parse(id, &text, registry).map_err(|e| format!("data/archive/{id}.toml: {e}"))
        })
        .collect()
}

fn parse(id: &str, text: &str, registry: &Registry) -> Result<Dataset, String> {
    let mut dataset: Dataset = toml::from_str(text).map_err(|e| e.to_string())?;
    dataset.id = id.to_string();
    if dataset.sources.is_empty() {
        return Err("give `sources`".into());
    }
    for id in &dataset.sources {
        let source = registry.sources.iter().find(|source| &source.id == id);
        let source = source.ok_or_else(|| format!("`{id}` is not a source of data/sources.toml"))?;
        if source.keep != Keep::Archive {
            return Err(format!("source `{id}` does not have `keep = \"archive\"`"));
        }
        if dataset.scheme.is_some_and(|scheme| scheme != source.version) {
            return Err("its sources have different `version` kinds".into());
        }
        dataset.scheme = Some(source.version);
        if let Some(version) = &dataset.version {
            crate::fetch::check_version(source, version)?;
        }
    }
    if dataset.kept == (!dataset.urls.is_empty() || !dataset.s3.is_empty()) {
        return Err("give `kept = true`, or else `urls`, `s3` or both".into());
    }
    let named = dataset.kept || matches!(dataset.scheme, Some(VersionScheme::Release | VersionScheme::Commit));
    if named && dataset.version.is_none() {
        return Err("give `version`".into());
    }
    if dataset.scheme == Some(VersionScheme::Digest) && (dataset.urls.len() != 1 || !dataset.s3.is_empty()) {
        return Err("a dataset of `digest` sources has one URL and no `s3`".into());
    }
    if let Some(url) = dataset.urls.iter().find(|url| archive::mirror_path(url).is_none()) {
        return Err(format!("`{url}` is not an `https://` URL of a plain path"));
    }
    if let Some(s3) = dataset.s3.iter().find(|s3| archive::mirror_path(&s3.bucket).is_none()) {
        return Err(format!("the bucket `{}` is not an `https://` URL of a plain path", s3.bucket));
    }
    Ok(dataset)
}

impl Dataset {
    /// The version of the dataset with `files`, got on `today`.
    fn version_of(&self, files: &[Listed], today: &str) -> String {
        if let Some(version) = &self.version {
            return version.clone();
        }
        match self.scheme {
            Some(VersionScheme::Digest) => files.first().and_then(|file| file.sha256.clone()).unwrap_or_default(),
            _ => files.iter().map(|file| file.modified.as_deref().unwrap_or(today)).max().unwrap_or(today).to_string(),
        }
    }
}

/// The client of a get of `dataset`: with the CDSE S3 keys under `home` when a bucket of the
/// dataset is on CDSE.
pub fn client(dataset: &Dataset, home: &Path) -> Result<Http, String> {
    let http = Http::new();
    if !dataset.s3.iter().any(|s3| s3.bucket.starts_with(CDSE)) {
        return Ok(http);
    }
    let values = crate::credentials::file_values(home, CDSE_KEYS);
    let need = |name: &str| {
        values.get(name).filter(|value| !value.is_empty()).cloned().ok_or_else(|| {
            format!("{CDSE_KEYS} has no {name}; set it with `obc data source credential {}`", dataset.sources[0])
        })
    };
    let keys = S3Keys {
        access_key_id: need("CDSE_S3_ACCESS_KEY")?,
        secret_access_key: need("CDSE_S3_SECRET_KEY")?,
        region: "default".into(),
    };
    Ok(http.with_s3(CDSE, keys))
}

/// A file that the listing names.
#[derive(Debug, PartialEq)]
struct Item {
    /// `None` for a file of a kept dataset.
    url: Option<String>,
    /// The path under the root.
    path: String,
    bytes: Option<u64>,
    etag: Option<String>,
    modified: Option<String>,
    /// Why upstream gave nothing about the file.
    error: Option<String>,
}

/// The files of `dataset`: those under `kept/<dataset>/` of `root` for a kept dataset.
fn list(http: &Http, dataset: &Dataset, root: &Path) -> Result<Vec<Item>, String> {
    let mut items = Vec::new();
    if dataset.kept {
        walk(&root.join("kept").join(&dataset.id), root, &mut items)?;
        items.sort_by(|a, b| a.path.cmp(&b.path));
    }
    for url in &dataset.urls {
        let path = archive::mirror_path(url).ok_or_else(|| format!("`{url}` names no mirror path"))?;
        let head = http.head(url).or_else(|e| if http::not_found(&e) { Err(e) } else { http.head(url) });
        let ((bytes, modified), error) = match head {
            Ok(head) => (head, None),
            Err(error) => ((None, None), Some(error)),
        };
        items.push(Item { url: Some(url.clone()), path, bytes, etag: None, modified, error });
    }
    for s3 in &dataset.s3 {
        let mut token = None;
        loop {
            let url = listing_url(s3, token.as_deref());
            let page = retried(|| http.listing(&url))?;
            let (objects, next) = page_items(&page, s3).map_err(|e| format!("{url}: {e}"))?;
            items.extend(objects);
            match next {
                Some(next) => token = Some(next),
                None => break,
            }
        }
    }
    let mut seen = HashSet::new();
    items.retain(|item| seen.insert(item.path.clone()));
    Ok(items)
}

fn walk(dir: &Path, root: &Path, items: &mut Vec<Item>) -> Result<(), String> {
    let Ok(entries) = fs::read_dir(dir) else { return Ok(()) };
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", dir.display()))?;
        let (path, kind) = (entry.path(), entry.file_type().map_err(|e| e.to_string())?);
        if kind.is_dir() {
            walk(&path, root, items)?;
        } else if kind.is_file() {
            let relative =
                path.strip_prefix(root).ok().and_then(|path| path.to_str()).filter(|path| archive::confined(path));
            let relative = relative.ok_or_else(|| format!("{}: not a plain UTF-8 path", path.display()))?;
            let bytes = entry.metadata().map_err(|e| e.to_string())?.len();
            let item =
                Item { url: None, path: relative.into(), bytes: Some(bytes), etag: None, modified: None, error: None };
            items.push(item);
        }
    }
    Ok(())
}

/// The URL of a page of a `ListObjectsV2`, with its query sorted by name as a signature needs it.
fn listing_url(s3: &S3, token: Option<&str>) -> String {
    let encode = |value: &str| crate::r2::s3::encode(value).replace('/', "%2F");
    let token = token.map(|token| format!("continuation-token={}&", encode(token))).unwrap_or_default();
    // A bucket without a path, in virtual-host style, lists at `/`.
    let slash = if s3.bucket["https://".len()..].contains('/') { "" } else { "/" };
    format!("{}{slash}?{token}list-type=2&prefix={}", s3.bucket, encode(&s3.prefix))
}

/// The objects of a page of a bucket listing that `s3` lists, and the token of the next page.
fn page_items(page: &str, s3: &S3) -> Result<(Vec<Item>, Option<String>), String> {
    let mut items = Vec::new();
    for contents in page.split("<Contents>").skip(1) {
        let key = tag(contents, "Key").ok_or("an object without a `Key`")?;
        let rest = key.strip_prefix(&s3.prefix).ok_or_else(|| format!("the key `{key}` is outside the prefix"))?;
        if key.ends_with('/') || !s3.pattern.as_deref().is_none_or(|pattern| matches(pattern, rest)) {
            continue;
        }
        let url = format!("{}/{}", s3.bucket, crate::r2::s3::encode(&key));
        let path = archive::mirror_path(&url).ok_or_else(|| format!("the key `{key}` names no plain path"))?;
        let bytes = Some(tag(contents, "Size").and_then(|size| size.parse().ok()).ok_or("an object without a `Size`")?);
        let modified = tag(contents, "LastModified").and_then(|time| time.get(..10).map(str::to_string));
        let modified = modified.filter(|day| date::parse(day).is_some());
        items.push(Item { url: Some(url), path, bytes, etag: tag(contents, "ETag"), modified, error: None });
    }
    let next = match tag(page, "IsTruncated").as_deref() {
        Some("true") => Some(tag(page, "NextContinuationToken").ok_or("a truncated page without a next token")?),
        _ => None,
    };
    Ok((items, next))
}

/// The text of the first element `name` of `xml`.
fn tag(xml: &str, name: &str) -> Option<String> {
    let start = xml.find(&format!("<{name}>"))? + name.len() + 2;
    let end = start + xml[start..].find(&format!("</{name}>"))?;
    let text = &xml[start..end];
    Some(
        text.replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&amp;", "&"),
    )
}

/// Whether `text` matches `pattern`, in which `*` stands for characters other than `/`.
fn matches(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, tail)) => text.strip_prefix(head).is_some_and(|rest| {
            let run = rest.find('/').unwrap_or(rest.len());
            (0..=run).filter(|&at| rest.is_char_boundary(at)).any(|at| matches(tail, &rest[at..]))
        }),
    }
}

fn retried<T>(mut attempt: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut wait = Duration::from_secs(2);
    for _ in 1..TRIES {
        match attempt() {
            Ok(value) => return Ok(value),
            Err(error) => eprintln!("obc data: {error} — retrying"),
        }
        std::thread::sleep(wait);
        wait *= 2;
    }
    attempt()
}

fn manifest_path(root: &Path, id: &str) -> PathBuf {
    root.join("manifest").join(format!("{id}.json"))
}

/// The first root that has the manifest of the dataset `id`.
pub fn holder<'a>(roots: &'a [PathBuf], id: &str) -> Option<&'a PathBuf> {
    roots.iter().find(|root| manifest_path(root, id).is_file())
}

/// The root of a get: `chosen`, which must be a root; else the root that has the manifest; else,
/// for a kept dataset, the root that has its files; else `None`, for the first root with room.
fn fixed_root(roots: &[PathBuf], dataset: &Dataset, chosen: Option<&Path>) -> Result<Option<PathBuf>, String> {
    if let Some(chosen) = chosen {
        let same =
            |root: &&PathBuf| root.as_path() == chosen || fs::canonicalize(root).ok() == fs::canonicalize(chosen).ok();
        let root =
            roots.iter().find(same).ok_or_else(|| format!("{} is not a root of OBC_DATA_ARCHIVE", chosen.display()))?;
        return Ok(Some(root.clone()));
    }
    if let Some(root) = holder(roots, &dataset.id) {
        return Ok(Some(root.clone()));
    }
    if !dataset.kept {
        return Ok(None);
    }
    let root = roots.iter().find(|root| root.join("kept").join(&dataset.id).is_dir());
    root.cloned()
        .map(Some)
        .ok_or_else(|| format!("no root has kept/{}/: put the files of the dataset there", dataset.id))
}

/// The first root with the free space for `needed` bytes and [`store::RESERVE`] more.
fn roomy(roots: &[PathBuf], needed: u64) -> Result<PathBuf, String> {
    for root in roots.iter().filter(|root| root.is_dir()) {
        if store::free_bytes(root)?.is_none_or(|(_, free)| free >= needed.saturating_add(store::RESERVE)) {
            return Ok(root.clone());
        }
    }
    Err(format!(
        "no root of OBC_DATA_ARCHIVE has {} free and 4 GiB more; give --root",
        crate::engine::runs::bytes(needed)
    ))
}

/// Hold the lock of the dataset `id` in `root`: one get or verify of a dataset at a time.
fn lock(root: &Path, id: &str) -> Result<fs::File, String> {
    let path = root.join(".partial").join(format!("{id}.lock"));
    fs::create_dir_all(root.join(".partial")).map_err(|e| format!("{}: {e}", path.display()))?;
    let file = fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    file.try_lock().map_err(|_| format!("another `obc data archive` command works on `{id}` in {}", root.display()))?;
    Ok(file)
}

/// Read the manifest of the dataset in `root`. In a manifest of the tools before `archive get`,
/// which has no `got`, the modification time of a file is its upstream `Last-Modified`.
fn read_old(root: &Path, id: &str) -> Result<Manifest, String> {
    let path = manifest_path(root, id);
    if !path.is_file() {
        return Ok(Manifest::default());
    }
    let mut manifest = Manifest::read(&path)?;
    if manifest.got.is_none() {
        for file in manifest.files.iter_mut().filter(|file| file.modified.is_none()) {
            let modified = fs::metadata(root.join(&file.path)).and_then(|metadata| metadata.modified()).ok();
            let seconds = modified.and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
            file.modified = seconds.map(|seconds| date::format((seconds.as_secs() / 86_400) as i64));
        }
    }
    Ok(manifest)
}

/// Get each listed file of `dataset` that its root does not hold, and write its manifest. Gives
/// the root and the manifest; the caller fails when the manifest has missing files.
pub fn get(
    http: &Http,
    dataset: &Dataset,
    roots: &[PathBuf],
    chosen: Option<&Path>,
) -> Result<(PathBuf, Manifest), String> {
    let fixed = fixed_root(roots, dataset, chosen)?;
    let items = list(http, dataset, fixed.as_deref().unwrap_or(Path::new("")))?;
    let root = match fixed {
        Some(root) => root,
        None => roomy(roots, items.iter().filter_map(|item| item.bytes).sum())?,
    };
    if !root.is_dir() {
        return Err(format!("the archive root {} does not exist: mount its drive", root.display()));
    }
    let _lock = lock(&root, &dataset.id)?;
    let old = read_old(&root, &dataset.id)?;
    let partial = root.join(".partial").join(&dataset.id);
    let today = date::format(date::today());
    let previous: HashMap<&str, &Listed> = old.files.iter().map(|file| (file.path.as_str(), file)).collect();
    let results: Mutex<Vec<Option<Result<Listed, String>>>> = Mutex::new(items.iter().map(|_| None).collect());
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let save = || {
        let manifest = assemble(dataset, &items, &results.lock().unwrap(), &old, &root, &today);
        manifest.write(&manifest_path(&root, &dataset.id)).map(|()| manifest)
    };
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..WORKERS.min(items.len()))
            .map(|_| {
                scope.spawn(|| loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else { break };
                    let result = take(http, &root, &partial, item, previous.get(item.path.as_str()).copied());
                    if let Err(error) = &result {
                        eprintln!("obc data: archive {}: {}: {error}", dataset.id, item.path);
                    }
                    results.lock().unwrap()[index] = Some(result);
                    done.fetch_add(1, Ordering::Relaxed);
                })
            })
            .collect();
        let mut saved = Instant::now();
        while !workers.iter().all(|worker| worker.is_finished()) {
            std::thread::sleep(Duration::from_millis(200));
            if saved.elapsed() >= SAVE {
                if let Err(error) = save() {
                    eprintln!("obc data: warning: {error}");
                }
                eprintln!(
                    "obc data: archive {}: {} of {} files",
                    dataset.id,
                    done.load(Ordering::Relaxed),
                    items.len()
                );
                saved = Instant::now();
            }
        }
    });
    Ok((root.clone(), save()?))
}

/// The file of `item` in `root`: the file of the manifest before, `old`, when it is in place and
/// upstream gives nothing else for it; else read in place for a kept file; else downloaded.
fn take(http: &Http, root: &Path, partial: &Path, item: &Item, old: Option<&Listed>) -> Result<Listed, String> {
    let file = root.join(&item.path);
    let same = |a: &Option<String>, b: &Option<String>| a.is_none() || b.is_none() || a == b;
    let unchanged = |old: &Listed| {
        item.bytes.is_none_or(|bytes| bytes == old.bytes)
            && same(&old.etag, &item.etag)
            && same(&old.modified, &item.modified)
            && old
                .md5
                .as_ref()
                .zip(item.etag.as_ref())
                .is_none_or(|(md5, etag)| etag.contains('-') || etag.trim_matches('"') == md5)
    };
    if let Some(old) = old.filter(|old| in_place(root, old) && (item.error.is_some() || unchanged(old))) {
        let modified = old.modified.clone().or_else(|| item.modified.clone());
        return Ok(Listed { modified, etag: old.etag.clone().or_else(|| item.etag.clone()), md5: None, ..old.clone() });
    }
    if let Some(error) = &item.error {
        return Err(error.clone());
    }
    let Some(url) = &item.url else {
        let (sha256, bytes) = store::hash_file(&file)?;
        return Ok(Listed {
            path: item.path.clone(),
            bytes,
            sha256: Some(sha256),
            modified: None,
            etag: None,
            md5: None,
        });
    };
    let part = partial.join(http::key(url));
    let expect =
        Expect { sha256: None, modified_by: item.modified.as_deref(), etag: item.etag.as_deref(), bytes: item.bytes };
    let got = http.download_to(&part, url, &expect)?;
    let parent = file.parent().ok_or("a mirror path has a directory")?;
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    fs::rename(&part, &file).map_err(|e| format!("{}: {e}", file.display()))?;
    let _ = fs::remove_file(part.with_extension("lock"));
    eprintln!("obc data: archive got {url}");
    let (modified, etag) = (item.modified.clone(), item.etag.clone());
    Ok(Listed { path: item.path.clone(), bytes: got.size, sha256: Some(got.sha256), modified, etag, md5: None })
}

/// Whether the file of `listed` is in `root` with its listed size.
fn in_place(root: &Path, listed: &Listed) -> bool {
    fs::metadata(root.join(&listed.path)).is_ok_and(|metadata| metadata.is_file() && metadata.len() == listed.bytes)
}

/// The manifest of the files got so far. A file that is not done yet, or that the listing no
/// longer names, keeps its entry of the manifest before while it is in place.
fn assemble(
    dataset: &Dataset,
    items: &[Item],
    results: &[Option<Result<Listed, String>>],
    old: &Manifest,
    root: &Path,
    today: &str,
) -> Manifest {
    let previous: HashMap<&str, &Listed> = old.files.iter().map(|file| (file.path.as_str(), file)).collect();
    let (mut files, mut missing) = (Vec::new(), Vec::new());
    let mut miss = |item: &Item, error: &str| {
        missing.push(Missing { path: item.path.clone(), bytes: item.bytes, error: error.to_string() })
    };
    for (item, result) in items.iter().zip(results) {
        match result {
            Some(Ok(listed)) => files.push(listed.clone()),
            Some(Err(error)) => miss(item, error),
            None => match previous.get(item.path.as_str()).filter(|listed| in_place(root, listed)) {
                Some(listed) => files.push(Listed { md5: None, ..(*listed).clone() }),
                None => miss(item, "not got yet"),
            },
        }
    }
    let named: HashSet<&str> = items.iter().map(|item| item.path.as_str()).collect();
    let kept = old.files.iter().filter(|file| !named.contains(file.path.as_str()) && in_place(root, file));
    files.extend(kept.map(|file| Listed { md5: None, ..file.clone() }));
    Manifest {
        dataset: dataset.id.clone(),
        sources: dataset.sources.clone(),
        name: dataset.name.clone(),
        coverage: dataset.coverage.clone(),
        notes: dataset.notes.clone(),
        version: Some(dataset.version_of(&files, today)),
        got: Some(today.to_string()),
        files,
        missing,
        clms_dataset_uid: dataset.clms.as_ref().map(|clms| clms.dataset.clone()),
        clms_file_id: dataset.clms.as_ref().map(|clms| clms.file.clone()),
    }
}

/// Read each file of the manifest of the dataset `id` and compare its size and SHA-256. A file
/// that differs moves to `missing`. Gives the files that differ.
pub fn verify(roots: &[PathBuf], id: &str) -> Result<Vec<Missing>, String> {
    let root = holder(roots, id).ok_or_else(|| format!("no root of OBC_DATA_ARCHIVE has manifest/{id}.json"))?;
    let _lock = lock(root, id)?;
    let path = manifest_path(root, id);
    let mut manifest = Manifest::read(&path)?;
    let (mut bad, mut reported, count) = (Vec::new(), Instant::now(), manifest.files.len());
    let mut checked = 0;
    manifest.files.retain(|listed| {
        checked += 1;
        if reported.elapsed() >= Duration::from_secs(30) {
            eprintln!("obc data: archive {id}: verified {checked} of {count} files");
            reported = Instant::now();
        }
        let error = differs(root, listed);
        if let Some(error) = &error {
            eprintln!("obc data: archive {id}: {}: {error}", listed.path);
            bad.push(Missing { path: listed.path.clone(), bytes: Some(listed.bytes), error: error.clone() });
        }
        error.is_none()
    });
    if !bad.is_empty() {
        manifest.missing.extend(bad.iter().cloned());
        manifest.write(&path)?;
    }
    Ok(bad)
}

/// Why the file of `listed` in `root` is not the one of the manifest.
fn differs(root: &Path, listed: &Listed) -> Option<String> {
    if !in_place(root, listed) {
        return Some(format!("not in place with {} bytes", listed.bytes));
    }
    match (store::hash_file(&root.join(&listed.path)), &listed.sha256) {
        (Err(error), _) => Some(error),
        (Ok((sha256, _)), Some(want)) if !want.eq_ignore_ascii_case(&sha256) => {
            Some(format!("the SHA-256 is {sha256}, but the manifest lists {want}"))
        }
        _ => None,
    }
}

/// The output of `obc data archive status`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ArchiveStatus {
    /// The roots of `OBC_DATA_ARCHIVE`, in order.
    pub roots: Vec<ArchiveRoot>,
    /// Each dataset of `data/archive/`, then each manifest without a definition.
    pub datasets: Vec<ArchiveDataset>,
    /// The sources with `keep = "archive"` that no dataset has.
    pub unarchived: Vec<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct ArchiveRoot {
    pub path: String,
    /// `null` when the root does not exist.
    pub free_bytes: Option<u64>,
}

/// A dataset of the archive.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ArchiveDataset {
    pub dataset: String,
    pub name: String,
    /// Ids of `data/sources.toml`.
    pub sources: Vec<String>,
    pub coverage: String,
    /// The root that has its manifest.
    pub root: Option<String>,
    pub state: DatasetState,
    /// Why the manifest is `unreadable`.
    pub error: Option<String>,
    pub version: Option<String>,
    /// The day of the last `archive get`.
    pub got: Option<String>,
    /// The files that the last get listed.
    pub listed: usize,
    /// The listed files that are in the root.
    pub present: usize,
    /// The files of the manifest that are in the root with their size: they serve.
    pub verified: usize,
    /// The listed files that are not in the root.
    pub missing: usize,
    /// The bytes of the verified files.
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum DatasetState {
    /// Each listed file is verified.
    Complete,
    /// A listed file is missing.
    Incomplete,
    /// No root has the manifest.
    Absent,
    /// The manifest cannot be read, or has no valid `version`.
    Unreadable,
    /// A manifest that no file of `data/archive/` defines.
    Undefined,
}

/// The status of each dataset of `datasets` and of each manifest without a definition.
pub fn status(datasets: &[Dataset], registry: &Registry, roots: &[PathBuf]) -> ArchiveStatus {
    let free =
        |root: &PathBuf| root.is_dir().then(|| store::free_bytes(root).ok().flatten().map(|(_, free)| free)).flatten();
    let roots_status =
        roots.iter().map(|root| ArchiveRoot { path: root.display().to_string(), free_bytes: free(root) });
    let defined: HashSet<&str> = datasets.iter().map(|dataset| dataset.id.as_str()).collect();
    let mut undefined: Vec<String> = roots
        .iter()
        .flat_map(|root| fs::read_dir(root.join("manifest")).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "json"))
        .filter_map(|path| path.file_stem().and_then(|stem| stem.to_str()).map(str::to_string))
        .filter(|id| !defined.contains(id.as_str()))
        .collect();
    undefined.sort();
    undefined.dedup();
    let rows = datasets.iter().map(|dataset| row(&dataset.id, Some(dataset), registry, roots));
    let named: HashSet<&str> = datasets.iter().flat_map(|dataset| dataset.sources.iter().map(String::as_str)).collect();
    ArchiveStatus {
        roots: roots_status.collect(),
        datasets: rows.chain(undefined.iter().map(|id| row(id, None, registry, roots))).collect(),
        unarchived: registry
            .sources
            .iter()
            .filter(|source| source.keep == Keep::Archive && !named.contains(source.id.as_str()))
            .map(|source| source.id.clone())
            .collect(),
    }
}

/// The status of the dataset `id`, with its definition when it has one.
pub fn row(id: &str, dataset: Option<&Dataset>, registry: &Registry, roots: &[PathBuf]) -> ArchiveDataset {
    let root = holder(roots, id);
    let mut row = ArchiveDataset {
        dataset: id.to_string(),
        name: dataset.map(|dataset| dataset.name.clone()).unwrap_or_default(),
        sources: dataset.map(|dataset| dataset.sources.clone()).unwrap_or_default(),
        coverage: dataset.map(|dataset| dataset.coverage.clone()).unwrap_or_default(),
        root: root.map(|root| root.display().to_string()),
        state: DatasetState::Absent,
        error: None,
        version: None,
        got: None,
        listed: 0,
        present: 0,
        verified: 0,
        missing: 0,
        bytes: 0,
    };
    let Some(root) = root else { return row };
    let manifest = match Manifest::read(&manifest_path(root, id)) {
        Ok(manifest) => manifest,
        Err(error) => {
            (row.state, row.error) = (DatasetState::Unreadable, Some(error));
            return row;
        }
    };
    if dataset.is_none() {
        (row.name, row.sources, row.coverage) = (manifest.name, manifest.sources, manifest.coverage);
    }
    let size = |path: &str| fs::metadata(root.join(path)).ok().filter(|metadata| metadata.is_file()).map(|m| m.len());
    for file in &manifest.files {
        match size(&file.path) {
            Some(bytes) if bytes == file.bytes => {
                (row.verified, row.present, row.bytes) = (row.verified + 1, row.present + 1, row.bytes + bytes)
            }
            Some(_) => row.present += 1,
            None => {}
        }
    }
    row.present += manifest.missing.iter().filter(|missing| size(&missing.path).is_some()).count();
    row.listed = manifest.files.len() + manifest.missing.len();
    row.missing = row.listed - row.present;
    (row.version, row.got) = (manifest.version, manifest.got);
    let source = dataset.and_then(|dataset| registry.sources.iter().find(|source| source.id == dataset.sources[0]));
    let given = dataset.and_then(|dataset| dataset.version.as_ref());
    let invalid = match (&row.version, source) {
        (None, _) => Some("no `version`".to_string()),
        (Some(version), _) if given.is_some_and(|given| given != version) => Some(format!(
            "the version is `{version}`, but data/archive/{id}.toml gives `{}`",
            given.map_or("", String::as_str)
        )),
        (Some(version), Some(source)) => crate::fetch::check_version(source, version).err(),
        (Some(_), None) => None,
    };
    row.state = match (dataset, invalid) {
        (None, _) => DatasetState::Undefined,
        (Some(_), Some(error)) => {
            row.error = Some(error);
            DatasetState::Unreadable
        }
        _ if row.listed > 0 && row.verified == row.listed => DatasetState::Complete,
        _ => DatasetState::Incomplete,
    };
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::tests::{quick, serve, whole};
    use crate::store::tests::Scratch;

    fn repository() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    #[test]
    fn the_datasets_of_the_repository_hold_and_a_broken_one_is_refused() {
        let registry = Registry::load(&repository()).unwrap();
        let datasets = load(&repository(), &registry).unwrap();
        assert!(datasets.iter().any(|dataset| dataset.id == "copernicus-glo-30"));
        let cases = [
            (r#"sources = ["no-such-source"]"#, "is not a source"),
            (r#"sources = ["geofabrik-poly"]"#, r#"does not have `keep = "archive"`"#),
            (r#"sources = ["hansen-gfc"]"#, "give `version`"),
            (r#"sources = ["egdi-geology"]"#, "one URL"),
        ];
        for (sources, error) in cases {
            let urls = r#"urls = ["https://example.org/a", "https://example.org/b"]"#;
            let text = format!("name = \"x\"\ncoverage = \"World\"\n{urls}\n{sources}\n");
            let refused = parse("x", &text, &registry).map(|_| ()).unwrap_err();
            assert!(refused.contains(error), "{sources}: {refused}");
        }
    }

    #[test]
    fn a_page_of_a_listing_gives_the_matching_objects_and_the_next_token() {
        let s3 =
            S3 { bucket: "https://bucket.example".into(), prefix: "t/".into(), pattern: Some("*/*_DEM.tif".into()) };
        let object = |key: &str| {
            format!("<Contents><Key>{key}</Key><LastModified>2022-05-09T14:26:37.000Z</LastModified><ETag>&quot;ab&quot;</ETag><Size>7</Size></Contents>")
        };
        let page = format!(
            "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>n/x=</NextContinuationToken>{}{}{}</ListBucketResult>",
            object("t/A_DEM/A_DEM.tif"),
            object("t/A_DEM/AUXFILES/A_EDM_DEM.tif"),
            object("t/A_DEM/"),
        );
        let (items, next) = page_items(&page, &s3).unwrap();
        let expected = Item {
            url: Some("https://bucket.example/t/A_DEM/A_DEM.tif".into()),
            path: "mirror/bucket.example/t/A_DEM/A_DEM.tif".into(),
            bytes: Some(7),
            etag: Some("\"ab\"".into()),
            modified: Some("2022-05-09".into()),
            error: None,
        };
        assert_eq!((items, next.as_deref()), (vec![expected], Some("n/x=")));
        assert_eq!(
            listing_url(&s3, Some("n/x=")),
            "https://bucket.example/?continuation-token=n%2Fx%3D&list-type=2&prefix=t%2F"
        );
    }

    #[test]
    fn a_get_takes_over_the_old_manifest_downloads_a_changed_file_and_verify_drops_a_bad_one() {
        let scratch = Scratch::new("datasets-get");
        let (actual, log) = serve(|_, _| whole(b"land"));
        let url = actual.replacen("http://", "https://", 1);
        let registry = Registry::load(&repository()).unwrap();
        let text = format!("name = \"x\"\nsources = [\"land-polygons\"]\ncoverage = \"World\"\nurls = [\"{url}\"]\n");
        let dataset = parse("x", &text, &registry).unwrap();
        let root = scratch.0.join("archive");
        let path = archive::mirror_path(&url).unwrap();
        std::fs::create_dir_all(root.join(&path).parent().unwrap()).unwrap();
        std::fs::create_dir_all(root.join("manifest")).unwrap();
        std::fs::write(root.join(&path), b"land").unwrap();
        // A manifest of the old tools, whose file has the upstream `Last-Modified` as its time.
        let old = format!(
            r#"{{"version": "file mtime", "files": [{{"path": "{path}", "bytes": 4, "sha256": "{}"}}]}}"#,
            store::sha256_hex(b"land")
        );
        std::fs::write(root.join("manifest/x.json"), old).unwrap();
        let noon =
            std::time::UNIX_EPOCH + Duration::from_secs(date::parse("2026-10-05").unwrap() as u64 * 86_400 + 43_200);
        std::fs::File::options().write(true).open(root.join(&path)).unwrap().set_modified(noon).unwrap();
        let roots = vec![root.clone()];

        let (_, manifest) = get(&Http::loopback(&actual), &dataset, &roots, None).unwrap();
        let requests = || log.lock().unwrap().len();
        assert_eq!(requests(), 1, "only the HEAD: the old file is the upstream file");
        assert_eq!(
            (manifest.version.as_deref(), manifest.files[0].modified.as_deref()),
            (Some("2026-10-05"), Some("2026-10-05"))
        );
        assert_eq!(row("x", Some(&dataset), &registry, &roots).state, DatasetState::Complete);

        std::fs::write(root.join(&path), b"lane").unwrap();
        let bad = verify(&roots, "x").unwrap();
        assert_eq!(bad.len(), 1, "{bad:?}");
        let state = row("x", Some(&dataset), &registry, &roots);
        assert_eq!((state.state, state.present, state.verified), (DatasetState::Incomplete, 1, 0));
        let (_, manifest) = get(&Http::loopback(&actual), &dataset, &roots, None).unwrap();
        assert_eq!(std::fs::read(root.join(&path)).unwrap(), b"land", "the get downloads the bad file again");
        assert_eq!((manifest.files.len(), manifest.missing.len(), requests()), (1, 0, 3));
        assert!(verify(&roots, "x").unwrap().is_empty());
    }

    #[test]
    fn a_kept_dataset_lists_the_files_under_its_folder_without_a_request() {
        let scratch = Scratch::new("datasets-kept");
        let root = scratch.0.join("archive");
        std::fs::create_dir_all(root.join("kept/x/a")).unwrap();
        std::fs::write(root.join("kept/x/a/1.tif"), b"one").unwrap();
        let registry = Registry::load(&repository()).unwrap();
        let text = "name = \"x\"\nsources = [\"corine-land-cover\"]\ncoverage = \"Europe\"\nversion = \"2026-10-28\"\nkept = true\n";
        let dataset = parse("x", text, &registry).unwrap();
        let roots = vec![scratch.0.join("absent"), root.clone()];
        let (chosen, manifest) = get(&quick(), &dataset, &roots, None).unwrap();
        assert_eq!(chosen, root, "the root that has kept/x/");
        let listed = &manifest.files[0];
        assert_eq!((listed.path.as_str(), listed.sha256.clone()), ("kept/x/a/1.tif", Some(store::sha256_hex(b"one"))));
        assert_eq!(manifest.version.as_deref(), Some("2026-10-28"));
        let status = status(std::slice::from_ref(&dataset), &registry, &roots);
        assert_eq!(status.roots[0].free_bytes, None, "a root that does not exist");
        assert_eq!(status.datasets[0].state, DatasetState::Complete);
    }
}
