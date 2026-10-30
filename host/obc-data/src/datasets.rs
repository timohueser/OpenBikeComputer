//! The datasets of the archive: their definitions in `data/archive/<dataset>.toml`, and how
//! `obc data archive` lists, gets and verifies them in a root of `OBC_DATA_ARCHIVE`. The fetchers
//! read the archive through [`crate::fetch::archive`].

use std::collections::HashMap;
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
use crate::store::{self, Store};

/// The CDSE S3 API: its requests are signed with the keys of [`CDSE_KEYS`].
const CDSE: &str = "https://eodata.dataspace.copernicus.eu";
const CDSE_KEYS: &str = "~/.config/openbikecomputer/cdse-s3.env";
/// Downloads at once. Chosen, not measured: the tools before this one ran 4 to 6.
const WORKERS: usize = 4;
/// How often a get writes the manifest while it runs.
const SAVE: Duration = Duration::from_secs(60);

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
    /// The bytes of its files from a listing of upstream, while no manifest lists them.
    pub bytes_estimate: Option<u64>,
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
    toml_paths(&dir, entries.map(|entry| entry.map(|entry| entry.path())))?
        .iter()
        .map(|path| {
            let id = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
            let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
            parse(id, &text, registry).map_err(|e| format!("data/archive/{id}.toml: {e}"))
        })
        .collect()
}

/// The `*.toml` paths of `entries` of `dir`, sorted. An entry that cannot be read fails, so that no
/// dataset leaves the catalog without an error.
fn toml_paths(dir: &Path, entries: impl Iterator<Item = std::io::Result<PathBuf>>) -> Result<Vec<PathBuf>, String> {
    let mut paths = Vec::new();
    for path in entries {
        let path = path.map_err(|e| format!("{}: {e}", dir.display()))?;
        if path.extension().is_some_and(|e| e == "toml") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
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
    /// The version of the dataset with `files`: `before` is the version of the manifest before,
    /// which a date dataset keeps while no file gives a day.
    fn version_of(&self, files: &[Listed], before: Option<&str>, today: &str) -> String {
        if let Some(version) = &self.version {
            return version.clone();
        }
        let found = match self.scheme {
            Some(VersionScheme::Digest) => files.first().map(|file| file.sha256.clone()),
            _ => files.iter().filter_map(|file| file.modified.clone()).max(),
        };
        found.or_else(|| before.map(str::to_string)).unwrap_or_else(|| today.to_string())
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
#[derive(Debug, Default, PartialEq)]
struct Item {
    /// `None` for a file of a kept dataset.
    url: Option<String>,
    /// The path under the root.
    path: String,
    bytes: Option<u64>,
    /// The `ETag` of a bucket listing, which a download sends as `If-Match`.
    etag: Option<String>,
    /// The MD5 that a bucket listing gives as the `ETag` of a file of one part.
    md5: Option<String>,
    modified: Option<String>,
    /// The `Last-Modified` time, in seconds since 1970-01-01: a mirrored file gets it as its time.
    modified_at: Option<u64>,
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
        let item = Item { url: Some(url.clone()), path, ..Item::default() };
        items.push(match http.head(url).or_else(|e| if http::not_found(&e) { Err(e) } else { http.head(url) }) {
            Ok(head) => Item { bytes: head.bytes, modified: head.modified, modified_at: head.modified_at, ..item },
            Err(error) => Item { error: Some(error), ..item },
        });
    }
    for s3 in &dataset.s3 {
        items.extend(list_s3(s3, |url| http.listing(url))?);
    }
    let mut seen = std::collections::HashSet::new();
    items.retain(|item| seen.insert(item.path.clone()));
    Ok(items)
}

fn walk(dir: &Path, root: &Path, items: &mut Vec<Item>) -> Result<(), String> {
    // Only a folder that is not there has no files: a folder that cannot be read fails the get.
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
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
            items.push(Item { path: relative.into(), bytes: Some(bytes), ..Item::default() });
        }
    }
    Ok(())
}

/// The objects that `s3` lists, from the pages that `page` gets.
fn list_s3(s3: &S3, mut page: impl FnMut(&str) -> Result<String, String>) -> Result<Vec<Item>, String> {
    let (mut items, mut token) = (Vec::new(), None);
    loop {
        let url = listing_url(s3, token.as_deref());
        let (objects, next) = page_items(&page(&url)?, s3).map_err(|e| format!("{url}: {e}"))?;
        items.extend(objects);
        match next {
            Some(next) => token = Some(next),
            None => return Ok(items),
        }
    }
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
        let Some(path) = archive::mirror_path(&url) else {
            eprintln!("obc data: warning: the key `{key}` names no plain path; it is left out");
            continue;
        };
        let bytes = Some(tag(contents, "Size").and_then(|size| size.parse().ok()).ok_or("an object without a `Size`")?);
        let modified_at = tag(contents, "LastModified").and_then(|time| date::seconds(&time));
        let modified = modified_at.map(|seconds| date::format((seconds / 86_400) as i64));
        let etag = tag(contents, "ETag");
        let md5 = etag.as_deref().and_then(md5_of);
        items.push(Item { url: Some(url), path, bytes, etag, md5, modified, modified_at, error: None });
    }
    let next = match tag(page, "IsTruncated").as_deref() {
        Some("true") => Some(tag(page, "NextContinuationToken").ok_or("a truncated page without a next token")?),
        _ => None,
    };
    Ok((items, next))
}

/// The MD5 of a file of one part, which S3 and GCS give as its `ETag` in a bucket listing.
fn md5_of(etag: &str) -> Option<String> {
    let hex = etag.trim_matches('"');
    (hex.len() == 32 && hex.bytes().all(|b| b.is_ascii_hexdigit())).then(|| hex.to_ascii_lowercase())
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

fn manifest_path(root: &Path, id: &str) -> PathBuf {
    root.join("manifest").join(format!("{id}.json"))
}

/// The first root that has the manifest of the dataset `id`.
pub fn holder<'a>(roots: &'a [PathBuf], id: &str) -> Option<&'a PathBuf> {
    roots.iter().find(|root| manifest_path(root, id).is_file())
}

/// The root of a get: `chosen`; else the root that has the manifest; else, for a kept dataset, the
/// root that has its files; else `None`, for the first root with room. A dataset is in one root,
/// so the get refuses while a root is not there.
fn fixed_root(roots: &[PathBuf], dataset: &Dataset, chosen: Option<&Path>) -> Result<Option<PathBuf>, String> {
    mounted(roots)?;
    // A get that stopped before its first manifest leaves its partial work in its root.
    let partial = |root: &&PathBuf| root.join(".partial").join(&dataset.id).is_dir();
    let holder = holder(roots, &dataset.id).or_else(|| roots.iter().find(partial));
    if let Some(chosen) = chosen {
        let chosen = fs::canonicalize(chosen).map_err(|e| format!("{}: {e}", chosen.display()))?;
        let root = roots.iter().find(|root| fs::canonicalize(root).is_ok_and(|root| root == chosen));
        let root = root.ok_or_else(|| format!("{} is not a root of OBC_DATA_ARCHIVE", chosen.display()))?;
        if let Some(holder) = holder.filter(|holder| *holder != root) {
            return Err(format!("{} holds `{}`: a dataset is in one root", holder.display(), dataset.id));
        }
        if dataset.kept && holder.is_none() && !root.join("kept").join(&dataset.id).is_dir() {
            return Err(format!(
                "{} has no kept/{}/: put the files of the dataset there, or give the root that has them",
                root.display(),
                dataset.id
            ));
        }
        return Ok(Some(root.clone()));
    }
    if let Some(root) = holder {
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

/// Fail while a root does not exist: it can hold what a command writes.
pub(crate) fn mounted(roots: &[PathBuf]) -> Result<(), String> {
    match roots.iter().find(|root| !root.is_dir()) {
        Some(root) => {
            Err(format!("the archive root {} does not exist: create the directory or mount its drive", root.display()))
        }
        None => Ok(()),
    }
}

/// The first root with the free space for `needed` bytes and [`store::RESERVE`] more.
fn roomy(roots: &[PathBuf], needed: u64) -> Result<PathBuf, String> {
    for root in roots {
        if store::free_bytes(root)?.is_none_or(|(_, free)| free >= needed.saturating_add(store::RESERVE)) {
            return Ok(root.clone());
        }
    }
    Err(format!(
        "no root of OBC_DATA_ARCHIVE has {} free and 4 GiB more; give --root",
        crate::engine::runs::bytes(needed)
    ))
}

/// The lock of the dataset `id` in the store: one get or verify of a dataset at a time.
fn lock(store: &Store, id: &str) -> Result<store::Lock, String> {
    store
        .try_lock(&format!("archive-{id}"))?
        .ok_or_else(|| format!("another `obc data archive` command works on `{id}`"))
}

/// What a get did.
pub struct Got {
    pub root: PathBuf,
    pub manifest: Manifest,
    /// Each file that the get could not take. A file of the manifest before stays in it.
    pub errors: Vec<Missing>,
}

/// Get each listed file of `dataset` that its root does not hold, and write its manifest.
pub fn get(
    http: &Http,
    store: &Store,
    dataset: &Dataset,
    roots: &[PathBuf],
    chosen: Option<&Path>,
) -> Result<Got, String> {
    let fixed = fixed_root(roots, dataset, chosen)?;
    let items = list(http, dataset, fixed.as_deref().unwrap_or(Path::new("")))?;
    let root = match fixed {
        Some(root) => root,
        None => roomy(roots, items.iter().filter_map(|item| item.bytes).sum())?,
    };
    let _lock = lock(store, &dataset.id)?;
    let path = manifest_path(&root, &dataset.id);
    let old = if path.is_file() { Manifest::read(&path)? } else { Manifest::default() };
    let partial = root.join(".partial").join(&dataset.id);
    let today = date::format(date::today());
    let previous: HashMap<&str, &Listed> = old.files.iter().map(|file| (file.path.as_str(), file)).collect();
    let items = with_lost_kept_files(dataset, items, &old);
    let recorded = recorded(dataset, &old);
    let results: Mutex<Vec<Option<Result<Listed, String>>>> = Mutex::new(items.iter().map(|_| None).collect());
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let save = || {
        let manifest = assemble(dataset, &items, &results.lock().unwrap(), &old, &recorded, &root, &today);
        manifest.write(&path).map(|()| manifest)
    };
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..WORKERS.min(items.len()))
            .map(|_| {
                scope.spawn(|| loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(index) else { break };
                    let old = previous.get(item.path.as_str()).copied();
                    let recorded = recorded.get(item.path.as_str()).copied();
                    let result = take(http, &root, &partial, item, old, recorded);
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
    let manifest = save()?;
    let results = results.into_inner().unwrap();
    let errors = items
        .iter()
        .zip(results)
        .filter_map(|(item, result)| match result {
            Some(Err(error)) => Some(Missing { path: item.path.clone(), bytes: item.bytes, sha256: None, error }),
            _ => None,
        })
        .collect();
    Ok(Got { root, manifest, errors })
}

/// Whether upstream gives for `item` what the manifest entry `old` recorded: the same size, and
/// the same `ETag` and `Last-Modified` day where upstream gives them.
fn unchanged(old: &Listed, item: &Item) -> bool {
    item.bytes.is_none_or(|bytes| bytes == old.bytes)
        && item.etag.as_ref().is_none_or(|etag| old.etag.as_ref() == Some(etag))
        && item.modified.as_ref().is_none_or(|day| old.modified.as_ref() == Some(day))
}

/// The recorded SHA-256 of each file of a kept dataset, of its `files` and of its `missing`.
fn recorded<'a>(dataset: &Dataset, old: &'a Manifest) -> HashMap<&'a str, &'a str> {
    let files = old.files.iter().map(|file| (file.path.as_str(), file.sha256.as_str()));
    let missing = old.missing.iter().filter_map(|file| Some((file.path.as_str(), file.sha256.as_deref()?)));
    files.chain(missing).filter(|_| dataset.kept).collect()
}

/// `items`, and for a kept dataset each file under `kept/<dataset>/` that the manifest records and
/// the listing does not find: it stays in the manifest as missing.
fn with_lost_kept_files(dataset: &Dataset, mut items: Vec<Item>, old: &Manifest) -> Vec<Item> {
    let folder = format!("kept/{}/", dataset.id);
    let found: std::collections::HashSet<String> = items.iter().map(|item| item.path.clone()).collect();
    let paths = old.files.iter().map(|file| &file.path).chain(old.missing.iter().map(|file| &file.path));
    let lost: Vec<Item> = paths
        .filter(|path| dataset.kept && path.starts_with(&folder) && !found.contains(*path))
        .map(|path| Item { path: path.clone(), error: Some("the kept file is not in place".into()), ..Item::default() })
        .collect();
    items.extend(lost);
    items
}

/// The file of `item` in `root`:
/// - the entry `old` of the manifest, when its file is in place and upstream did not change it;
/// - for a kept file, the bytes of its `recorded` SHA-256, or else read in place when the manifest
///   has no entry for it: a get never gives a kept file other bytes than the manifest records;
/// - a file on disk whose size and modification day are the `Content-Length` and the
///   `Last-Modified` day of upstream, read in place once;
/// - else downloaded.
fn take(
    http: &Http,
    root: &Path,
    partial: &Path,
    item: &Item,
    old: Option<&Listed>,
    recorded: Option<&str>,
) -> Result<Listed, String> {
    let file = root.join(&item.path);
    if let Some(old) = old.filter(|old| in_place(root, old) && unchanged(old, item)) {
        return Ok(old.clone());
    }
    if let Some(error) = &item.error {
        return Err(error.clone());
    }
    let listed = |sha256, bytes| Listed {
        path: item.path.clone(),
        bytes,
        sha256,
        modified: item.modified.clone(),
        etag: item.etag.clone(),
    };
    let Some(url) = &item.url else {
        let (sha256, bytes) = store::hash_file(&file)?;
        return match recorded.filter(|want| !want.eq_ignore_ascii_case(&sha256)) {
            Some(want) => Err(format!(
                "the SHA-256 is {sha256}, but the manifest records {want}; to replace a kept file, remove its entry from the manifest"
            )),
            None => Ok(listed(sha256, bytes)),
        };
    };
    if upstream_size_and_day(&file, item) {
        let (sha256, md5, bytes) = store::digests(&file)?;
        if item.md5.as_ref().is_none_or(|want| *want == md5) {
            return Ok(listed(sha256, bytes));
        }
        eprintln!("obc data: {}: the MD5 is {md5}, not the one of upstream; downloading it again", file.display());
    }
    let part = partial.join(http::key(url));
    let expect = Expect {
        sha256: None,
        modified_by: item.modified.as_deref(),
        etag: item.etag.as_deref(),
        bytes: item.bytes,
        md5: item.md5.as_deref(),
    };
    let got = http.download_to(&part, url, &expect)?;
    // The modification time of an archive file is the upstream `Last-Modified`.
    if let Some(seconds) = item.modified_at {
        let time = std::time::UNIX_EPOCH + Duration::from_secs(seconds);
        let opened = fs::File::options().write(true).open(&part);
        opened.and_then(|part| part.set_modified(time)).map_err(|e| format!("{}: {e}", part.display()))?;
    }
    let parent = file.parent().ok_or("a mirror path has a directory")?;
    fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    fs::rename(&part, &file).map_err(|e| format!("{}: {e}", file.display()))?;
    let _ = fs::remove_file(part.with_extension("lock"));
    eprintln!("obc data: archive got {url}");
    Ok(listed(got.sha256, got.size))
}

/// Whether `file` has the size and the modification day that upstream gives for `item`, and
/// upstream gives at least one of them.
fn upstream_size_and_day(file: &Path, item: &Item) -> bool {
    let Some(metadata) = fs::metadata(file).ok().filter(|metadata| metadata.is_file()) else { return false };
    let seconds = metadata.modified().ok().and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    let day = seconds.map(|seconds| date::format((seconds.as_secs() / 86_400) as i64));
    (item.bytes.is_some() || item.modified.is_some())
        && item.bytes.is_none_or(|bytes| bytes == metadata.len())
        && item.modified.as_ref().is_none_or(|modified| day.as_ref() == Some(modified))
}

/// Whether the file of `listed` is in `root` with its listed size.
fn in_place(root: &Path, listed: &Listed) -> bool {
    fs::metadata(root.join(&listed.path)).is_ok_and(|metadata| metadata.is_file() && metadata.len() == listed.bytes)
}

/// The manifest of the files got so far. A file that is not done yet, or that the get could not
/// take, keeps its entry of the manifest before while that file is in place.
fn assemble(
    dataset: &Dataset,
    items: &[Item],
    results: &[Option<Result<Listed, String>>],
    old: &Manifest,
    recorded: &HashMap<&str, &str>,
    root: &Path,
    today: &str,
) -> Manifest {
    let previous: HashMap<&str, &Listed> = old.files.iter().map(|file| (file.path.as_str(), file)).collect();
    let (mut files, mut missing) = (Vec::new(), Vec::new());
    for (item, result) in items.iter().zip(results) {
        match (result, previous.get(item.path.as_str()).filter(|listed| in_place(root, listed))) {
            (Some(Ok(listed)), _) => files.push(listed.clone()),
            (_, Some(listed)) => files.push((*listed).clone()),
            (result, None) => missing.push(Missing {
                path: item.path.clone(),
                bytes: item.bytes,
                sha256: recorded.get(item.path.as_str()).map(|sha256| sha256.to_string()),
                error: match result {
                    Some(Err(error)) => error.clone(),
                    _ => "not got yet".into(),
                },
            }),
        }
    }
    Manifest {
        dataset: dataset.id.clone(),
        sources: dataset.sources.clone(),
        name: dataset.name.clone(),
        coverage: dataset.coverage.clone(),
        notes: dataset.notes.clone(),
        version: Some(dataset.version_of(&files, old.version.as_deref(), today)),
        got: Some(today.to_string()),
        files,
        missing,
        clms_dataset_uid: dataset.clms.as_ref().map(|clms| clms.dataset.clone()),
        clms_file_id: dataset.clms.as_ref().map(|clms| clms.file.clone()),
    }
}

/// Read each file of the manifest of the dataset `id` and compare its size and SHA-256. A file
/// that differs moves to `missing`; a mirrored one also moves to `.partial/<id>/rejected/`, so the
/// next get downloads it again. A kept file stays in place, as it has no other source. Gives the
/// files that differ.
pub fn verify(store: &Store, roots: &[PathBuf], id: &str) -> Result<Vec<Missing>, String> {
    let root = holder(roots, id).ok_or_else(|| format!("no root of OBC_DATA_ARCHIVE has manifest/{id}.json"))?;
    let _lock = lock(store, id)?;
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
        let Some(error) = differs(root, listed) else { return true };
        eprintln!("obc data: archive {id}: {}: {error}", listed.path);
        let kept = listed.path.starts_with("kept/");
        let file = root.join(&listed.path);
        if !kept && file.is_file() {
            let aside = root.join(".partial").join(id).join("rejected").join(&listed.path);
            let moved = aside.parent().map_or(Ok(()), fs::create_dir_all).and_then(|()| fs::rename(&file, &aside));
            if let Err(e) = moved {
                eprintln!("obc data: warning: {}: {e}", file.display());
            }
        }
        let sha256 = kept.then(|| listed.sha256.clone());
        bad.push(Missing { path: listed.path.clone(), bytes: Some(listed.bytes), sha256, error });
        false
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
    match store::hash_file(&root.join(&listed.path)) {
        Err(error) => Some(error),
        Ok((sha256, _)) if !listed.sha256.eq_ignore_ascii_case(&sha256) => {
            Some(format!("the SHA-256 is {sha256}, but the manifest lists {}", listed.sha256))
        }
        Ok(_) => None,
    }
}

/// The output of `obc data archive status`.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ArchiveStatus {
    /// The roots of `OBC_DATA_ARCHIVE`, in order.
    pub roots: Vec<ArchiveRoot>,
    /// Each dataset of `data/archive/`.
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
    /// Their bytes, as far as the manifest knows them.
    pub listed_bytes: u64,
    /// The files of the manifest that are in the root with their size: they serve.
    pub verified: usize,
    /// The bytes of the verified files.
    pub bytes: u64,
    /// The `bytes_estimate` of the definition.
    pub bytes_estimate: Option<u64>,
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
    /// The manifest cannot be read, or has no `version`: no file of it serves.
    Unreadable,
}

/// The status of each dataset of `datasets`.
pub fn status(datasets: &[Dataset], registry: &Registry, roots: &[PathBuf]) -> ArchiveStatus {
    let free =
        |root: &PathBuf| root.is_dir().then(|| store::free_bytes(root).ok().flatten().map(|(_, free)| free)).flatten();
    let named: std::collections::HashSet<&str> =
        datasets.iter().flat_map(|dataset| dataset.sources.iter().map(String::as_str)).collect();
    ArchiveStatus {
        roots: roots
            .iter()
            .map(|root| ArchiveRoot { path: root.display().to_string(), free_bytes: free(root) })
            .collect(),
        datasets: datasets.iter().map(|dataset| row(dataset, roots)).collect(),
        unarchived: registry
            .sources
            .iter()
            .filter(|source| source.keep == Keep::Archive && !named.contains(source.id.as_str()))
            .map(|source| source.id.clone())
            .collect(),
    }
}

/// The status of `dataset`.
pub fn row(dataset: &Dataset, roots: &[PathBuf]) -> ArchiveDataset {
    let root = holder(roots, &dataset.id);
    let mut row = ArchiveDataset {
        dataset: dataset.id.clone(),
        sources: dataset.sources.clone(),
        coverage: dataset.coverage.clone(),
        root: root.map(|root| root.display().to_string()),
        state: DatasetState::Absent,
        error: None,
        version: None,
        got: None,
        listed: 0,
        listed_bytes: 0,
        verified: 0,
        bytes: 0,
        bytes_estimate: dataset.bytes_estimate,
    };
    let Some(root) = root else { return row };
    let manifest = match Manifest::read(&manifest_path(root, &dataset.id)) {
        Ok(Manifest { version: None, .. }) => Err("no `version`".to_string()),
        read => read,
    };
    let manifest = match manifest {
        Ok(manifest) => manifest,
        Err(error) => {
            (row.state, row.error) = (DatasetState::Unreadable, Some(error));
            return row;
        }
    };
    for file in manifest.files.iter().filter(|file| in_place(root, file)) {
        (row.verified, row.bytes) = (row.verified + 1, row.bytes + file.bytes);
    }
    row.listed = manifest.files.len() + manifest.missing.len();
    let missing = manifest.missing.iter().filter_map(|missing| missing.bytes);
    row.listed_bytes = manifest.files.iter().map(|file| file.bytes).chain(missing).sum();
    (row.version, row.got) = (manifest.version, manifest.got);
    // A get that lists no file names no dataset: a wrong prefix or an empty folder.
    row.state =
        if row.listed > 0 && row.verified == row.listed { DatasetState::Complete } else { DatasetState::Incomplete };
    row
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fetch::tests::{not_found, serve, whole};
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
    fn an_entry_of_the_catalog_that_cannot_be_read_fails_the_load() {
        let dir = Path::new("data/archive");
        let entries = [Ok(dir.join("b.toml")), Ok(dir.join("notes.txt")), Ok(dir.join("a.toml"))];
        let paths = toml_paths(dir, entries.into_iter()).unwrap();
        assert_eq!(paths, [dir.join("a.toml"), dir.join("b.toml")]);
        let broken = [Ok(dir.join("a.toml")), Err(std::io::Error::other("bad entry"))];
        let err = toml_paths(dir, broken.into_iter()).unwrap_err();
        assert!(err.contains("bad entry"), "{err}");
    }

    #[test]
    fn a_listing_follows_its_pages_and_gives_the_matching_objects() {
        let s3 = S3 {
            bucket: "https://eodata.dataspace.copernicus.eu/eodata".into(),
            prefix: "t/".into(),
            pattern: Some("*/*_DEM.tif".into()),
        };
        let object = |key: &str, etag: &str| {
            format!("<Contents><Key>{key}</Key><LastModified>2022-05-09T14:26:37.000Z</LastModified><ETag>&quot;{etag}&quot;</ETag><Size>7</Size></Contents>")
        };
        let md5 = "0123456789abcdef0123456789abcdef";
        let first = format!(
            "<ListBucketResult><IsTruncated>true</IsTruncated><NextContinuationToken>n/x=</NextContinuationToken>{}{}{}</ListBucketResult>",
            object("t/A_DEM/A_DEM.tif", md5),
            object("t/A_DEM/AUXFILES/A_EDM_DEM.tif", md5),
            object("t/A_DEM/", md5),
        );
        let second = format!(
            "<ListBucketResult><IsTruncated>false</IsTruncated>{}</ListBucketResult>",
            object("t/B_DEM/B_DEM.tif", "ab-2")
        );
        let mut asked = Vec::new();
        let items = list_s3(&s3, |url| {
            asked.push(url.to_string());
            Ok(if asked.len() == 1 { first.clone() } else { second.clone() })
        })
        .unwrap();
        // The path style of CDSE lists at the bucket path, without a `/` before the query.
        let base = "https://eodata.dataspace.copernicus.eu/eodata?";
        assert_eq!(
            asked,
            [
                format!("{base}list-type=2&prefix=t%2F"),
                format!("{base}continuation-token=n%2Fx%3D&list-type=2&prefix=t%2F")
            ]
        );
        let expected = Item {
            url: Some("https://eodata.dataspace.copernicus.eu/eodata/t/A_DEM/A_DEM.tif".into()),
            path: "mirror/eodata.dataspace.copernicus.eu/eodata/t/A_DEM/A_DEM.tif".into(),
            bytes: Some(7),
            etag: Some(format!("\"{md5}\"")),
            md5: Some(md5.into()),
            modified: Some("2022-05-09".into()),
            modified_at: date::seconds("2022-05-09T14:26:37Z"),
            error: None,
        };
        assert_eq!(items[0], expected);
        assert_eq!((items.len(), items[1].md5.as_deref()), (2, None), "a file of more parts has no MD5");
        let virtual_host = S3 { bucket: "https://bucket.example".into(), prefix: String::new(), pattern: None };
        assert_eq!(listing_url(&virtual_host, None), "https://bucket.example/?list-type=2&prefix=");
    }

    #[test]
    fn an_entry_stays_only_while_upstream_gives_the_same_size_etag_and_day() {
        let old = Listed {
            path: "p".into(),
            bytes: 4,
            sha256: String::new(),
            modified: Some("2026-10-05".into()),
            etag: Some("\"a\"".into()),
        };
        let item =
            Item { bytes: Some(4), etag: Some("\"a\"".into()), modified: Some("2026-10-05".into()), ..Item::default() };
        assert!(unchanged(&old, &item));
        assert!(unchanged(&old, &Item::default()), "upstream gives nothing to compare");
        let newer = Item { modified: Some("2026-10-06".into()), ..Item::default() };
        let larger = Item { bytes: Some(5), ..Item::default() };
        let other = Item { etag: Some("\"b\"".into()), ..Item::default() };
        for changed in [newer, larger, other] {
            assert!(!unchanged(&old, &changed), "{changed:?}");
        }
        let undated = Listed { modified: None, ..old };
        assert!(!unchanged(&undated, &item), "an entry without a day is no proof");
    }

    #[test]
    fn a_get_downloads_reuses_keeps_an_entry_that_fails_and_reads_a_file_of_upstream_size_and_day() {
        let scratch = Scratch::new("datasets-get");
        // Requests 0 to 2 see the file of 5 October, the later ones that of 6 October; the GET of
        // request 4 fails.
        let (actual, log) = serve(|index, _| match index {
            4 => not_found(),
            _ => {
                let mut reply = whole(b"land");
                if index > 2 {
                    reply.headers.retain(|(name, _)| *name != "Last-Modified");
                    reply.headers.push(("Last-Modified", "Tue, 06 Oct 2026 03:43:59 GMT".into()));
                }
                reply
            }
        });
        let url = actual.replacen("http://", "https://", 1);
        let registry = Registry::load(&repository()).unwrap();
        let text = format!("name = \"x\"\nsources = [\"land-polygons\"]\ncoverage = \"World\"\nurls = [\"{url}\"]\n");
        let dataset = parse("x", &text, &registry).unwrap();
        let root = scratch.0.join("archive");
        std::fs::create_dir_all(&root).unwrap();
        let (roots, store) = (vec![root.clone()], Store::at(scratch.0.join("store")));
        let path = archive::mirror_path(&url).unwrap();
        let http = Http::loopback(&actual);
        let requests = || log.lock().unwrap().len();
        let day = |got: &Got| got.manifest.files[0].modified.clone().unwrap();

        let got = get(&http, &store, &dataset, &roots, None).unwrap();
        assert_eq!(
            (requests(), day(&got), got.manifest.version.as_deref()),
            (2, "2026-10-05".into(), Some("2026-10-05"))
        );
        get(&http, &store, &dataset, &roots, None).unwrap();
        assert_eq!(requests(), 3, "only the HEAD: upstream did not change the file");

        let got = get(&http, &store, &dataset, &roots, None).unwrap();
        assert_eq!((requests(), got.errors.len()), (5, 1), "upstream is newer, and its download fails");
        assert_eq!(day(&got), "2026-10-05", "the intact file of the manifest before stays");

        // Bad bytes with the upstream size and time: only the rejection keeps the get from them.
        std::fs::write(root.join(&path), b"lane").unwrap();
        let upstream = date::http_seconds("Tue, 06 Oct 2026 03:43:59 GMT").unwrap();
        set_time(&root.join(&path), upstream);
        assert_eq!(verify(&store, &roots, "x").unwrap().len(), 1);
        assert_eq!(std::fs::read(root.join(".partial/x/rejected").join(&path)).unwrap(), b"lane", "set aside");
        assert_eq!(row(&dataset, &roots).state, DatasetState::Incomplete);
        let got = get(&http, &store, &dataset, &roots, None).unwrap();
        assert_eq!(std::fs::read(root.join(&path)).unwrap(), b"land", "the get downloads the bad file again");
        assert_eq!((requests(), day(&got), got.errors.len()), (7, "2026-10-06".into(), 0));
        assert_eq!(time(&root.join(&path)), upstream, "the exact upstream `Last-Modified`");

        std::fs::remove_file(root.join("manifest/x.json")).unwrap();
        let got = get(&http, &store, &dataset, &roots, None).unwrap();
        assert_eq!(
            (requests(), got.manifest.files.len()),
            (8, 1),
            "the file of upstream size and day is read in place"
        );
        assert!(verify(&store, &roots, "x").unwrap().is_empty());
    }

    fn time(file: &Path) -> u64 {
        let modified = std::fs::metadata(file).unwrap().modified().unwrap();
        modified.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
    }

    fn set_time(file: &Path, seconds: u64) {
        let time = std::time::UNIX_EPOCH + Duration::from_secs(seconds);
        std::fs::File::options().write(true).open(file).unwrap().set_modified(time).unwrap();
    }

    #[test]
    fn a_kept_dataset_lists_the_files_under_its_folder_and_a_dataset_stays_in_its_root() {
        let scratch = Scratch::new("datasets-kept");
        let (first, root) = (scratch.0.join("first"), scratch.0.join("archive"));
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(root.join("kept/x/a")).unwrap();
        std::fs::write(root.join("kept/x/a/1.tif"), b"one").unwrap();
        let registry = Registry::load(&repository()).unwrap();
        let text = "name = \"x\"\nsources = [\"corine-land-cover\"]\ncoverage = \"Europe\"\nversion = \"2026-10-28\"\nkept = true\n";
        let dataset = parse("x", text, &registry).unwrap();
        let (roots, store) = (vec![first.clone(), root.clone()], Store::at(scratch.0.join("store")));
        let http = Http::with_backoff(Duration::ZERO);
        let err = get(&http, &store, &dataset, &roots, Some(&first)).map(|_| ()).unwrap_err();
        assert!(err.contains("has no kept/x/"), "{err}");
        let got = get(&http, &store, &dataset, &roots, None).unwrap();
        assert_eq!(got.root, root, "the root that has kept/x/");
        let listed = &got.manifest.files[0];
        assert_eq!((listed.path.as_str(), listed.sha256.clone()), ("kept/x/a/1.tif", store::sha256_hex(b"one")));
        assert_eq!(got.manifest.version.as_deref(), Some("2026-10-28"));

        std::fs::write(root.join("kept/x/a/1.tif"), b"oNe").unwrap();
        assert_eq!(verify(&store, &roots, "x").unwrap().len(), 1);
        let got = get(&http, &store, &dataset, &roots, None).unwrap();
        assert_eq!(
            (got.errors.len(), got.manifest.files.len()),
            (1, 0),
            "a get never blesses other bytes of a kept file"
        );
        std::fs::write(root.join("kept/x/a/1.tif"), b"one").unwrap();
        let got = get(&http, &store, &dataset, &roots, None).unwrap();
        assert_eq!((got.errors.len(), got.manifest.files.len()), (0, 1), "the recorded bytes take it back");
        let err = get(&http, &store, &dataset, &roots, Some(&first)).map(|_| ()).unwrap_err();
        assert!(err.contains("a dataset is in one root"), "{err}");
        let gone = vec![scratch.0.join("unmounted"), root.clone()];
        let err = get(&http, &store, &dataset, &gone, None).map(|_| ()).unwrap_err();
        assert!(err.contains("mount its drive"), "{err}");
        let status = status(std::slice::from_ref(&dataset), &registry, &gone);
        assert_eq!((status.roots[0].free_bytes, status.datasets[0].state), (None, DatasetState::Complete));
    }

    #[test]
    fn a_kept_folder_that_cannot_be_read_fails_the_get_and_an_empty_listing_is_incomplete() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new("datasets-unreadable");
        let root = scratch.0.join("archive");
        let locked = root.join("kept/x/locked");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("1.tif"), b"one").unwrap();
        let registry = Registry::load(&repository()).unwrap();
        let text = "name = \"x\"\nsources = [\"corine-land-cover\"]\ncoverage = \"Europe\"\nversion = \"2026-10-28\"\nkept = true\n";
        let dataset = parse("x", text, &registry).unwrap();
        let (roots, store) = (vec![root.clone()], Store::at(scratch.0.join("store")));
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        let result = get(&Http::with_backoff(Duration::ZERO), &store, &dataset, &roots, None).map(|_| ());
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(result.unwrap_err().contains("locked"), "a folder that cannot be read is no empty folder");

        std::fs::create_dir_all(root.join("manifest")).unwrap();
        std::fs::write(root.join("manifest/x.json"), r#"{"version": "2026-10-28", "files": []}"#).unwrap();
        assert_eq!(row(&dataset, &roots).state, DatasetState::Incomplete, "a listing without files");
    }
}
