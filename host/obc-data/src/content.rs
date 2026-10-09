//! Manually prepared content snapshots. Bakes resolve exact identities from the selected pin.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::fetch::capture;
use crate::r2::{Bucket, Upload};
use crate::store::{self, FileRecord, Snapshot, Store};

mod preparation;
pub use preparation::prepare;
pub(crate) use preparation::transform_images;

pub const SOURCE: &str = "wikimedia-content";

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub config: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Selected {
    pub sha256: String,
}

fn settings(store: &Store, name: &str) -> PathBuf {
    store.root().join("settings").join(name)
}

pub fn selected(store: &Store) -> Result<Option<Selected>, String> {
    let path = settings(store, "content-source.json");
    match fs::read(&path) {
        Ok(bytes) => {
            let selected: Selected = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
            check_digest(&selected.sha256)?;
            Ok(Some(selected))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

pub fn configure(store: &Store, path: &Path) -> Result<Request, String> {
    let path = path.canonicalize().map_err(|e| e.to_string())?;
    let mut config: Value = read(&path)?;
    let base = path.parent().ok_or("configuration has no parent")?;
    let absolute_archive = |source: &mut Value| {
        if let Some(path) = source["path"].as_str() {
            source["path"] = json!(base.join(path));
        }
    };
    absolute_archive(&mut config["wikidata"]);
    for name in ["wikidata_pages", "wikidata_redirects"] {
        if let Some(source) = config.get_mut(name) {
            absolute_archive(source);
        }
    }
    for source in
        config["wikipedia"].as_object_mut().ok_or("configuration needs wikipedia language snapshots")?.values_mut()
    {
        absolute_archive(source);
    }
    if let Some(sources) = config.get_mut("langlinks").and_then(Value::as_object_mut) {
        for source in sources.values_mut() {
            absolute_archive(source);
        }
    }
    for path in config.get_mut("osm").and_then(Value::as_array_mut).into_iter().flatten() {
        if let Some(value) = path.as_str() {
            *path = json!(base.join(value));
        }
    }
    let request = Request { config };
    check_request(&request)?;
    store::durable(&settings(store, "content-config.json"), &serde_json::to_vec(&request).map_err(|e| e.to_string())?)?;
    Ok(request)
}

pub fn configured(store: &Store) -> Result<Request, String> {
    let request: Request = serde_json::from_slice(
        &fs::read(settings(store, "content-config.json")).map_err(|_| "configure content snapshot inputs first")?,
    )
    .map_err(|e| e.to_string())?;
    check_request(&request)?;
    Ok(request)
}

pub fn check_request(request: &Request) -> Result<(), String> {
    let config = &request.config;
    if !config.is_object() || !config["wikidata"].is_object() || !config["wikipedia"].is_object() {
        return Err("content configuration needs wikidata and wikipedia snapshots".into());
    }
    let archives = std::iter::once(&config["wikidata"])
        .chain(config["wikipedia"].as_object().unwrap().values())
        .chain(["wikidata_pages", "wikidata_redirects"].into_iter().filter_map(|name| config.get(name)))
        .chain(config.get("langlinks").and_then(Value::as_object).into_iter().flat_map(|sources| sources.values()));
    if config.get("wikidata_redirects").is_some() && config.get("wikidata_pages").is_none() {
        return Err("Wikidata redirect proofs need the matching page SQL snapshot".into());
    }
    for archive in archives {
        if archive["path"].as_str().is_some() == archive["url"].as_str().is_some()
            || archive["date"].as_str().and_then(crate::date::parse).is_none()
        {
            return Err("each archive needs its snapshot date and either path or HTTPS URL".into());
        }
        if let Some(url) = archive["url"].as_str() {
            if !url.starts_with("https://") {
                return Err("snapshot downloads require HTTPS".into());
            }
        }
    }
    if config["osm"].as_array().is_none_or(Vec::is_empty)
        && config["entities"].as_array().is_none_or(Vec::is_empty)
        && config["links"].as_array().is_none_or(Vec::is_empty)
    {
        return Err("content preparation needs OSM inputs or explicit identities".into());
    }
    Ok(())
}

fn read(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?).map_err(|e| e.to_string())
}

fn python(root: &Path, args: &[&str], paths: &[(&str, &Path)]) -> Result<(), String> {
    let mut command = capture::python(root, None)?;
    command.arg("tools/wikimedia_snapshot.py").args(args);
    for (flag, path) in paths {
        command.arg(flag).arg(path);
    }
    if let Some(run) = crate::cli::operation_cli::current_run() {
        command.env("OBC_CONTENT_CONTROL", crate::operation::directory(&Store::open()?, &run)?.join("control.json"));
    }
    let status = command.stdout(std::io::stderr()).status().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("content snapshot step failed: {status}; successful work is retained"));
    }
    Ok(())
}

fn check_digest(digest: &str) -> Result<(), String> {
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        return Err("content version must be a lowercase SHA-256".into());
    }
    Ok(())
}

pub fn manifest(store: &Store, digest: &str) -> Result<Value, String> {
    check_digest(digest)?;
    let path = store.object(digest);
    if store::hash_file(&path)?.0 != digest {
        return Err("content manifest checksum changed".into());
    }
    let manifest = read(&path)?;
    check_manifest(&manifest)?;
    Ok(manifest)
}

fn check_manifest(manifest: &Value) -> Result<(), String> {
    if manifest["schema"] != 1 || manifest["complete"] != true {
        return Err("content snapshot is incomplete".into());
    }
    let mut seen = BTreeSet::new();
    let mut indexes = 0;
    for file in manifest["files"].as_array().ok_or("content snapshot has no files")? {
        let digest = file["sha256"].as_str().ok_or("content file has no digest")?;
        check_digest(digest)?;
        let name = file["name"].as_str().ok_or("content file has no name")?;
        if (file["kind"] == "index" && name != "index.sqlite") || (file["kind"] != "index" && name != digest) {
            return Err("content file name differs from its kind or digest".into());
        }
        if !seen.insert(digest) || file["bytes"].as_u64().is_none() {
            return Err("invalid content file descriptor".into());
        }
        match file["kind"].as_str() {
            Some("index") => indexes += 1,
            Some("bundle") if file["bytes"].as_u64().unwrap() <= 16 * 1024 * 1024 => {}
            Some("image") if file["bytes"] == 216 * 240 => {}
            _ => return Err("invalid content bundle kind or size".into()),
        }
    }
    if indexes != 1 {
        return Err("content snapshot needs exactly one lookup index".into());
    }
    Ok(())
}

pub fn admit(store: &Store, out: &Path, manifest: &Value) -> Result<Selected, String> {
    check_manifest(manifest)?;
    if read(&out.join("manifest.json"))? != *manifest {
        return Err("content manifest changed before admission".into());
    }
    let mut files = Vec::new();
    for file in manifest["files"].as_array().unwrap() {
        let name = file["name"].as_str().ok_or("content file has no name")?;
        if name != "index.sqlite" {
            check_digest(name)?;
        }
        let (digest, size) = store::hash_file(&out.join(name))?;
        if file["sha256"] != digest || file["bytes"] != size {
            return Err("content file checksum changed".into());
        }
        store.insert(&out.join(name), &digest)?;
        files.push(FileRecord {
            name: name.into(),
            url: format!("content:objects/{digest}"),
            sha256: digest,
            size,
            retrieved: crate::date::timestamp(crate::date::now()),
        });
    }
    let (digest, size) = store::hash_file(&out.join("manifest.json"))?;
    store.insert(&out.join("manifest.json"), &digest)?;
    files.push(FileRecord {
        name: "manifest.json".into(),
        url: format!("content:manifests/{digest}"),
        sha256: digest.clone(),
        size,
        retrieved: crate::date::timestamp(crate::date::now()),
    });
    store.put_snapshot(&Snapshot { source: SOURCE.into(), version: digest.clone(), files })?;
    Ok(Selected { sha256: digest })
}

pub fn use_local(store: &Store, digest: &str) -> Result<Selected, String> {
    manifest(store, digest)?;
    let selected = Selected { sha256: digest.into() };
    store::durable(
        &settings(store, "content-source.json"),
        &serde_json::to_vec(&selected).map_err(|e| e.to_string())?,
    )?;
    Ok(selected)
}

pub fn publish(store: &Store, bucket: &Bucket, digest: &str) -> Result<Value, String> {
    publish_checked(store, bucket, digest, None)
}

fn publish_checked(
    store: &Store,
    bucket: &Bucket,
    digest: &str,
    run: Option<&crate::engine::runs::Run>,
) -> Result<Value, String> {
    let manifest = manifest(store, digest)?;
    let upload = Upload {
        immutable: true,
        cache_control: Some("public, max-age=31536000, immutable"),
        content_type: Some("application/octet-stream"),
    };
    for file in manifest["files"].as_array().unwrap() {
        if let Some(run) = run {
            run.check_stop(store)?;
        }
        let sha = file["sha256"].as_str().unwrap();
        let path = store.object(sha);
        if store::hash_file(&path)? != (sha.to_owned(), file["bytes"].as_u64().unwrap()) {
            return Err("content publication file changed".into());
        }
        let key = format!("content/objects/{sha}");
        bucket.put(&path, &key, &upload)?;
        bucket.verify(&path, &key)?;
    }
    let key = format!("content/manifests/{digest}.json");
    if let Some(run) = run {
        run.check_stop(store)?;
    }
    bucket.put(&store.object(digest), &key, &Upload { content_type: Some("application/json"), ..upload })?;
    bucket.verify(&store.object(digest), &key)?;
    Ok(json!({"snapshot":digest,"manifest_key":key,"objects":manifest["files"].as_array().unwrap().len()+1}))
}

pub fn publish_operation(store: &Store, id: &str, request: &Request) -> Result<Value, String> {
    let digest = request.config["snapshot"].as_str().ok_or("content publication needs a reviewed digest")?;
    let run = crate::engine::runs::Run::attach(store, id, &crate::engine::runs::events(store, id)?)?;
    let result = (|| {
        let bucket = Bucket::from_env(crate::r2::Credentials::Main)?;
        publish_checked(store, &bucket, digest, Some(&run))
    })();
    run.finish(result.as_ref().err().map(String::as_str))?;
    result
}

pub fn restore(store: &Store, bucket: &Bucket, digest: &str) -> Result<Selected, String> {
    check_digest(digest)?;
    let _using = store.using()?;
    let path = store.partial(&format!("content-manifest-{digest}"));
    bucket.get(&format!("content/manifests/{digest}.json"), &path)?;
    if store::hash_file(&path)?.0 != digest {
        return Err("downloaded content manifest checksum changed".into());
    }
    let size = fs::metadata(&path).map_err(|e| e.to_string())?.len();
    let value = read(&path)?;
    check_manifest(&value)?;
    store.insert(&path, digest)?;
    let index = value["files"].as_array().unwrap().iter().find(|file| file["kind"] == "index").unwrap();
    object(store, bucket, index)?;
    let mut files = Vec::new();
    for file in value["files"].as_array().unwrap() {
        files.push(FileRecord {
            name: file["name"].as_str().ok_or("content file has no name")?.into(),
            url: format!("content:objects/{}", file["sha256"].as_str().unwrap()),
            sha256: file["sha256"].as_str().unwrap().into(),
            size: file["bytes"].as_u64().unwrap(),
            retrieved: crate::date::timestamp(crate::date::now()),
        });
    }
    files.push(FileRecord {
        name: "manifest.json".into(),
        url: format!("content:manifests/{digest}"),
        sha256: digest.into(),
        size,
        retrieved: crate::date::timestamp(crate::date::now()),
    });
    store.put_snapshot(&Snapshot { source: SOURCE.into(), version: digest.into(), files })?;
    use_local(store, digest)
}

fn object(store: &Store, bucket: &Bucket, file: &Value) -> Result<(), String> {
    let digest = file["sha256"].as_str().ok_or("content file has no digest")?;
    let size = file["bytes"].as_u64().ok_or("content file has no size")?;
    if store.object(digest).is_file() {
        if store::hash_file(&store.object(digest))? == (digest.into(), size) {
            return Ok(());
        }
        return Err("stored content object checksum changed".into());
    }
    store::check_free(store.root(), size)?;
    let path = store.partial(&format!("content-object-{digest}"));
    bucket.get(&format!("content/objects/{digest}"), &path)?;
    if store::hash_file(&path)? != (digest.into(), size) {
        return Err("downloaded content object checksum changed".into());
    }
    store.insert(&path, digest).map(|_| ())
}

pub(crate) fn resolve(root: &Path, store: &Store, work: &Path, request: &Value) -> Result<PathBuf, String> {
    let digest = request["snapshot"].as_str().ok_or("content request has no snapshot pin")?;
    let manifest = manifest(store, digest)?;
    let descriptions = manifest["files"].as_array().unwrap();
    if descriptions.iter().any(|file| file["kind"] == "image") {
        let transform =
            store::sha256_hex(&fs::read(root.join("host/obc-pack/src/landmarks/photo.rs")).map_err(|e| e.to_string())?);
        if manifest["photo_transform_sha256"] != transform {
            return Err("prepared photo transform changed; prepare and select a new content snapshot".into());
        }
    }
    let index = descriptions.iter().find(|file| file["kind"] == "index").unwrap();
    let index_path = store.object(index["sha256"].as_str().unwrap());
    if store::hash_file(&index_path)? != (index["sha256"].as_str().unwrap().into(), index["bytes"].as_u64().unwrap()) {
        return Err("content index checksum changed".into());
    }
    fs::create_dir_all(work).map_err(|e| e.to_string())?;
    let request_path = work.join("hydrate-request.json");
    fs::write(&request_path, serde_json::to_vec(request).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let missing_path = work.join("missing.json");
    let files_path = work.join("objects.json");
    loop {
        let files = descriptions
            .iter()
            .filter_map(|file| {
                let digest = file["sha256"].as_str()?;
                let path = store.object(digest);
                path.is_file().then_some((digest, path))
            })
            .collect::<BTreeMap<_, _>>();
        fs::write(&files_path, serde_json::to_vec(&files).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        python(
            root,
            &["hydrate"],
            &[
                ("--work", work),
                ("--index", &index_path),
                ("--files", &files_path),
                ("--requests", &request_path),
                ("--out", &missing_path),
            ],
        )?;
        let missing = read(&missing_path)?;
        let missing = missing["missing"].as_array().ok_or("content lookup has no missing list")?;
        if missing.is_empty() {
            return Ok(work.join("catalog.sqlite"));
        }
        let bucket = Bucket::from_env(crate::r2::Credentials::Main)?;
        for digest in missing.iter().filter_map(Value::as_str) {
            let file = descriptions
                .iter()
                .find(|file| file["sha256"] == digest)
                .ok_or("content index names an unregistered bundle")?;
            object(store, &bucket, file)?;
        }
    }
}
