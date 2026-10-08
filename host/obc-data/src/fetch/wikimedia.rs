//! Stable Wikimedia facts admitted through the source snapshots of the Store.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path};

use serde_json::{json, Value};

use super::{capture, check_version, merge, snapshot_lock, Request};
use crate::date;
use crate::store::{self, FileRecord, Snapshot, Store};

const SOURCES: [&str; 3] = ["wikidata", "wikipedia", "commons"];

fn owner(kind: &str) -> Result<usize, String> {
    match kind {
        "entity" => Ok(0),
        "article" | "link" => Ok(1),
        "commons" | "file" | "category" | "mediainfo" => Ok(2),
        _ => Err(format!("unknown Wikimedia fact kind: {kind}")),
    }
}

fn read(path: &Path) -> Result<Value, String> {
    serde_json::from_slice(&fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value[name].as_str().ok_or_else(|| format!("Wikimedia pin lacks {name}"))
}

fn verified(store: &Store, pin: &Value) -> Result<Value, String> {
    let digest = text(pin, "sha256")?;
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
        return Err("invalid Wikimedia pin digest".into());
    }
    let path = store.object(digest);
    if store::hash_file(&path)?.0 != digest {
        return Err(format!("Wikimedia Store object changed: {digest}"));
    }
    let value = read(&path)?;
    for field in ["kind", "key", "status", "identity", "revision"] {
        if value[field] != pin[field] {
            return Err(format!("Wikimedia pin {field} differs from its object"));
        }
    }
    if !matches!(text(pin, "status")?, "present" | "missing") {
        return Err("unresolved Wikimedia fact cannot be admitted".into());
    }
    owner(text(pin, "kind")?)?;
    Ok(value)
}

/// Reuse only facts whose object and complete snapshot manifest remain reachable.
pub fn inputs(store: &Store) -> Result<Vec<Value>, String> {
    let mut latest: BTreeMap<(String, String), Value> = BTreeMap::new();
    for source in SOURCES {
        for snapshot in store.snapshots(source)? {
            for file in snapshot.files.iter().filter(|file| file.name.starts_with("#content/manifest/")) {
                if !store.object(&file.sha256).is_file() {
                    continue;
                }
                let manifest = read(&store.object(&file.sha256))?;
                for pin in manifest["records"].as_array().ok_or("invalid Wikimedia snapshot manifest")? {
                    let digest = text(pin, "sha256")?;
                    if !snapshot.files.iter().any(|file| file.sha256 == digest) || !store.object(digest).is_file() {
                        continue;
                    }
                    verified(store, pin)?;
                    let key = (text(pin, "kind")?.to_owned(), text(pin, "key")?.to_owned());
                    if latest.get(&key).is_none_or(|old| old["checked_at"].as_str() < pin["checked_at"].as_str()) {
                        let mut pin = pin.clone();
                        let value = verified(store, &pin)?;
                        if value["kind"] == "file" {
                            let asset_digest = text(&value["asset"], "sha256")?;
                            if !snapshot.files.iter().any(|file| file.sha256 == asset_digest)
                                || !store.object(asset_digest).is_file()
                            {
                                continue;
                            }
                            if store::hash_file(&store.object(asset_digest))?.0 != asset_digest {
                                return Err("retained media digest changed".into());
                            }
                            pin["asset_path"] =
                                json!(std::path::absolute(store.object(asset_digest)).map_err(|e| e.to_string())?);
                        }
                        pin["path"] = json!(std::path::absolute(store.object(digest)).map_err(|e| e.to_string())?);
                        latest.insert(key, pin);
                    }
                }
            }
        }
    }
    Ok(latest.into_values().collect())
}

/// `content=<JSON>` names an exact, region-independent set of entity, link or asset identities.
pub(super) fn run(
    root: &Path,
    store: &Store,
    request: &Request,
    checks: Option<crate::fetch::Checks<'_>>,
) -> Result<Snapshot, String> {
    let [(name, body)] = request.params.as_slice() else {
        return Err("Wikimedia shared fetch takes content=<JSON>".into());
    };
    if name != "content" {
        return Err("Wikimedia shared fetch takes content=<JSON>".into());
    }
    let mut requested: Value = serde_json::from_str(body).map_err(|e| format!("content request: {e}"))?;
    if !requested.is_object() || requested.get("check_id").is_some() || requested.get("refresh").is_some() {
        return Err("content request must name identities, without operation fields".into());
    }
    let version = request.version.clone().unwrap_or_else(|| date::format(date::today()));
    check_version(request.source, &version)?;
    if let (Some(names), Some(snapshot)) =
        (store.requested(&request.source.id, &version, &request.params)?, store.snapshot(&request.source.id, &version)?)
    {
        let files: Vec<_> = snapshot.files.into_iter().filter(|file| names.contains(&file.name)).collect();
        if files.len() == names.len() && files.iter().all(|file| store.object(&file.sha256).is_file()) {
            return Ok(Snapshot { source: request.source.id.clone(), version, files });
        }
    }
    if version != date::format(date::today()) {
        return Err(format!("Wikimedia {version} facts are not pinned in the Store"));
    }
    super::check_owner(root, request.source, checks)?;
    let key = store::sha256_hex(body.as_bytes());
    let _lock = store.lock(&format!("wikimedia-{key}"))?;
    let _api_lock = store.lock("wikimedia-api")?;
    let staging = std::path::absolute(store.partial(&format!("wikimedia-{key}"))).map_err(|e| e.to_string())?;
    let (work, out) = (staging.join("work"), staging.join("out"));
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    requested["check_id"] = json!(format!("{}-{version}", &key[..32]));
    requested["refresh"] = json!(false);
    let requests_path = staging.join("requests.json");
    let inputs_path = staging.join("inputs.json");
    fs::write(&requests_path, serde_json::to_vec(&requested).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let registry = crate::sources::Registry::load(root)?;
    let fresh = inputs(store)?
        .into_iter()
        .filter(|pin| {
            let Ok(index) = owner(pin["kind"].as_str().unwrap_or_default()) else { return false };
            let Some(source) = registry.sources.iter().find(|source| source.id == SOURCES[index]) else { return false };
            match source.refresh {
                crate::sources::Refresh::Manual => true,
                crate::sources::Refresh::Days(_) if pin["kind"] == "file" => true,
                crate::sources::Refresh::Days(days) => pin["checked_at"]
                    .as_str()
                    .and_then(date::seconds)
                    .is_some_and(|checked| date::now().saturating_sub(checked) < u64::from(days) * 86_400),
            }
        })
        .collect::<Vec<_>>();
    fs::write(&inputs_path, serde_json::to_vec(&fresh).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let code = super::owner_code(request.source).code;
    let before = code.files(root)?;
    let mut command = capture::python(root, None)?;
    command
        .arg("tools/wikimedia_acquire.py")
        .arg("--work")
        .arg(&work)
        .arg("--out")
        .arg(&out)
        .arg("--requests")
        .arg(&requests_path)
        .arg("--inputs")
        .arg(&inputs_path);
    let status = command.stdout(std::io::stderr()).status().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!(
            "Wikimedia acquisition failed with {status}; successful facts remain in {}",
            work.display()
        ));
    }
    let mut manifest = read(&out.join("manifest.json"))?;
    if manifest["schema"] != 1
        || manifest["complete"] != true
        || manifest["failures"].as_array().is_none_or(|v| !v.is_empty())
    {
        return Err("Wikimedia acquisition is incomplete".into());
    }
    media(root, store, &out, &requested, &mut manifest)?;
    if code.files(root)? != before {
        return Err("acquisition code changed; prepare again".into());
    }
    let snapshot = admit(store, &out, &manifest, &request.params, &version, &request.source.id)?;
    fs::remove_dir_all(staging).map_err(|e| e.to_string())?;
    Ok(snapshot)
}

fn media(root: &Path, store: &Store, out: &Path, requested: &Value, manifest: &mut Value) -> Result<(), String> {
    let http = super::http::Http::new().limited(3_125_000);
    let mut records = manifest["records"].as_array().ok_or("invalid content records")?.clone();
    let mut assets = manifest["assets"].as_array().cloned().unwrap_or_default();
    let mut pending = Vec::new();
    for filename in requested["files"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        let key = format!("File:{}", filename.trim_start_matches("File:").replace('_', " "));
        let pin = records
            .iter()
            .find(|pin| pin["kind"] == "commons" && pin["key"] == key)
            .ok_or("file lacks Commons metadata")?;
        let metadata = read(&out.join(text(pin, "path")?))?;
        if metadata["status"] == "missing" {
            continue;
        }
        if records.iter().any(|pin| {
            pin["kind"] == "file"
                && pin["key"] == key
                && pin["revision"] == metadata["file_revision"]
                && pin["identity"] == metadata["identity"]
        }) {
            continue;
        }
        let checked_at = pin["checked_at"].clone();
        records.retain(|pin| pin["kind"] != "file" || pin["key"] != key);
        pending.push((key, metadata, checked_at));
    }
    let filenames = pending.iter().map(|(key, _, _)| json!(key)).collect::<Vec<_>>();
    if filenames.is_empty() {
        return Ok(());
    }
    let before = check_media(root, out, &filenames, &records)?;
    let mut downloaded = Vec::new();
    for (key, metadata, checked_at) in pending {
        let url = text(&metadata["imageinfo"], "thumburl")?;
        if !url.starts_with("https://upload.wikimedia.org/wikipedia/commons/thumb/")
            || metadata["imageinfo"]["thumbwidth"] != 500
        {
            return Err("Commons conversion input is not a standard 500 px thumbnail".into());
        }
        let _download_lock = super::http::Http::lock(store, url)?;
        let download = http.download(store, url, &super::http::Expect::default())?;
        let asset = json!({"path":format!("assets/{}",download.sha256),"sha256":download.sha256,"bytes":download.size,"url":url,"input":"thumbnail500"});
        let target = out.join(text(&asset, "path")?);
        fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        if !target.is_file() {
            fs::hard_link(&download.object, &target).map_err(|e| e.to_string())?;
        }
        downloaded.push((key, metadata, checked_at, asset));
    }
    let after = check_media(root, out, &filenames, &records)?;
    for (key, metadata, checked_at, asset) in downloaded {
        let value = json!({"kind":"file","key":key,"status":"present","identity":metadata["identity"],"revision":metadata["file_revision"],"filename":metadata["filename"],"asset":asset,"revision_before":before[&key],"revision_after":after[&key]});
        let data = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
        let digest = store::sha256_hex(&data);
        let path = format!("content/file/{digest}.json");
        let target = out.join(&path);
        fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(&target, &data).map_err(|e| e.to_string())?;
        let pin = json!({"kind":"file","key":key,"status":"present","identity":value["identity"],"revision":value["revision"],"checked_at":checked_at,"path":path,"sha256":digest});
        let work = out.parent().ok_or("missing operation root")?.join("work");
        let work_target = work.join(&path);
        fs::create_dir_all(work_target.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(work_target, data).map_err(|e| e.to_string())?;
        let asset_target = work.join(text(&asset, "path")?);
        fs::create_dir_all(asset_target.parent().unwrap()).map_err(|e| e.to_string())?;
        if !asset_target.is_file() {
            fs::hard_link(out.join(text(&asset, "path")?), asset_target).map_err(|e| e.to_string())?;
        }
        let journal = work
            .join("records")
            .join(text(requested, "check_id")?)
            .join(format!("{}.json", store::sha256_hex(format!("file:{key}").as_bytes())));
        fs::create_dir_all(journal.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::write(journal, serde_json::to_vec(&pin).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        records.push(pin);
        assets.push(asset);
    }
    manifest["records"] = json!(records);
    manifest["assets"] = json!(assets);
    Ok(())
}

fn check_media(
    root: &Path,
    out: &Path,
    filenames: &[Value],
    expected: &[Value],
) -> Result<BTreeMap<String, Value>, String> {
    let id = format!(
        "media-{}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos()
    );
    let check = out.parent().ok_or("missing operation root")?.join(&id);
    fs::create_dir_all(&check).map_err(|e| e.to_string())?;
    let request = check.join("requests.json");
    let inputs = check.join("inputs.json");
    fs::write(&request, serde_json::to_vec(&json!({"check_id":id,"commons":filenames})).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    fs::write(&inputs, b"[]").map_err(|e| e.to_string())?;
    let mut command = capture::python(root, None)?;
    let result = check.join("out");
    let status = command
        .arg("tools/wikimedia_acquire.py")
        .arg("--work")
        .arg(check.join("work"))
        .arg("--out")
        .arg(&result)
        .arg("--requests")
        .arg(&request)
        .arg("--inputs")
        .arg(&inputs)
        .stdout(std::io::stderr())
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("thumbnail revision check failed".into());
    }
    let fresh = read(&result.join("manifest.json"))?;
    if fresh["complete"] != true {
        return Err("thumbnail revision check incomplete".into());
    }
    let mut witnesses = BTreeMap::new();
    for pin in expected.iter().filter(|pin| pin["kind"] == "commons") {
        let key = text(pin, "key")?;
        if !filenames.iter().any(|name| {
            name.as_str()
                .is_some_and(|name| format!("File:{}", name.trim_start_matches("File:").replace('_', " ")) == key)
        }) {
            continue;
        }
        let current = fresh["records"]
            .as_array()
            .and_then(|records| records.iter().find(|record| record["kind"] == "commons" && record["key"] == key))
            .ok_or("thumbnail current metadata missing")?;
        let before = read(&out.join(text(pin, "path")?))?;
        let after = read(&result.join(text(current, "path")?))?;
        if before["identity"] != after["identity"]
            || before["file_revision"] != after["file_revision"]
            || before["revision"] != after["revision"]
        {
            return Err(format!("Commons file or credits changed while acquiring thumbnail: {key}; refresh its facts"));
        }
        witnesses.insert(
            key.into(),
            json!({"query":{"pages":{"compact":{"title":key,"imageinfo":[{"timestamp":after["file_revision"]["timestamp"],"sha1":after["file_revision"]["sha1"],"description_revision":after["revision"],"extmetadata":after["imageinfo"]["extmetadata"]}]}}}}),
        );
    }
    fs::remove_dir_all(check).map_err(|e| e.to_string())?;
    Ok(witnesses)
}

fn admit(
    store: &Store,
    out: &Path,
    manifest: &Value,
    params: &[(String, String)],
    version: &str,
    mine: &str,
) -> Result<Snapshot, String> {
    let mut files: [Vec<FileRecord>; 3] = std::array::from_fn(|_| Vec::new());
    let mut records: [Vec<Value>; 3] = std::array::from_fn(|_| Vec::new());
    for pin in manifest["records"].as_array().ok_or("Wikimedia manifest has no records")? {
        let index = owner(text(pin, "kind")?)?;
        let path = Path::new(text(pin, "path")?);
        if path.components().any(|part| !matches!(part, Component::Normal(_))) {
            return Err("Wikimedia fact path escapes operation".into());
        }
        let path = out.join(path);
        let (digest, size) = store::hash_file(&path)?;
        if digest != text(pin, "sha256")? {
            return Err("Wikimedia staged fact digest changed".into());
        }
        store.insert(&path, &digest)?;
        verified(store, pin)?;
        files[index].push(FileRecord {
            name: format!("#content/{}/{digest}.json", text(pin, "kind")?),
            url: format!("wikimedia:content/{}/{digest}", text(pin, "kind")?),
            sha256: digest,
            size,
            retrieved: text(pin, "checked_at")?.into(),
        });
        records[index].push(pin.clone());
    }
    for asset in manifest["assets"].as_array().into_iter().flatten() {
        let relative = text(asset, "path")?;
        if Path::new(relative).components().any(|part| !matches!(part, Component::Normal(_))) {
            return Err("asset path escapes operation".into());
        }
        let path = out.join(relative);
        let (digest, size) = store::hash_file(&path)?;
        if digest != text(asset, "sha256")? || asset["bytes"] != size {
            return Err("media input checksum changed".into());
        }
        store.insert(&path, &digest)?;
        files[2].push(FileRecord {
            name: format!("#content/assets/{digest}"),
            url: format!("{}#sha256={digest}", text(asset, "url")?),
            sha256: digest,
            size,
            retrieved: date::timestamp(date::now()),
        });
    }
    for index in 0..3 {
        let path = out.join(format!("snapshot-{index}.json"));
        fs::write(&path, serde_json::to_vec(&json!({"schema":1,"records":records[index]})).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let (digest, size) = store::hash_file(&path)?;
        store.insert(&path, &digest)?;
        files[index].push(FileRecord {
            name: format!("#content/manifest/{digest}.json"),
            url: format!("wikimedia:content/manifest/{digest}"),
            sha256: digest,
            size,
            retrieved: date::timestamp(date::now()),
        });
    }
    let mut locks = Vec::new();
    let mut snapshots = Vec::new();
    for (source, files) in SOURCES.into_iter().zip(&files) {
        locks.push(store.lock(&snapshot_lock(source, version))?);
        snapshots.push(merge(store, source, version, files)?);
    }
    for snapshot in snapshots.into_iter().flatten() {
        store.put_snapshot(&snapshot)?;
    }
    for (source, files) in SOURCES.into_iter().zip(&files) {
        store.put_requested(
            source,
            &store::Requested {
                version: version.into(),
                params: params.to_vec(),
                files: files.iter().map(|file| file.name.clone()).collect(),
            },
        )?;
    }
    let index = SOURCES.iter().position(|source| *source == mine).ok_or("invalid Wikimedia source")?;
    Ok(Snapshot { source: mine.into(), version: version.into(), files: std::mem::take(&mut files[index]) })
}

/// Restore published compact facts through the same admission path. Bundles contain no media bytes.
pub(crate) fn restore_published(store: &Store, copies: &crate::input_copy::Restore<'_>) -> Result<(), String> {
    let _using = store.using()?;
    for product in &copies.live.products {
        let Some((_, release)) = &product.release else { continue };
        for file in release
            .layers
            .iter()
            .flat_map(|layer| layer.client_files())
            .filter(|file| file.path.starts_with("shared-content/"))
        {
            let _lock = store.lock(&format!("wikimedia-bundle-{}", file.sha256))?;
            let key = format!("{}/objects/{}", product.prefix, file.sha256);
            let data = if store.object(&file.sha256).is_file() {
                fs::read(store.object(&file.sha256)).map_err(|e| e.to_string())?
            } else {
                copies.remote.get(&key)?.ok_or_else(|| format!("published content bundle missing: {key}"))?
            };
            if data.len() as u64 != file.size || store::sha256_hex(&data) != file.sha256 {
                return Err("published content bundle checksum changed".into());
            }
            let bundle: Value = serde_json::from_slice(&data).map_err(|e| e.to_string())?;
            if bundle["schema"] != 1 {
                return Err("unsupported published content bundle".into());
            }
            let out = store.partial(&format!("wikimedia-bundle-{}", file.sha256));
            fs::create_dir_all(&out).map_err(|e| e.to_string())?;
            let mut records = Vec::new();
            for entry in bundle["records"].as_array().ok_or("bundle records missing")? {
                let mut pin = entry["pin"].clone();
                let kind = text(&pin, "kind")?;
                if kind == "file" {
                    return Err("published bundle contains a local media input".into());
                }
                owner(kind)?;
                let payload = text(entry, "data")?;
                let digest = text(&pin, "sha256")?;
                if store::sha256_hex(payload.as_bytes()) != digest {
                    return Err("published fact checksum changed".into());
                }
                let relative = format!("content/{kind}/{digest}.json");
                let path = out.join(&relative);
                fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
                fs::write(path, payload).map_err(|e| e.to_string())?;
                pin["path"] = json!(relative);
                records.push(pin);
            }
            let manifest = json!({"schema":1,"complete":true,"failures":[],"records":records});
            let version = records
                .iter()
                .filter_map(|pin| pin["checked_at"].as_str().and_then(date::seconds))
                .max()
                .map(|seconds| date::format((seconds / 86_400) as i64))
                .ok_or("bundle has no fact check time")?;
            let params = vec![("content-bundle".into(), file.sha256.clone())];
            admit(store, &out, &manifest, &params, &version, "wikidata")?;
            fs::remove_dir_all(out).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn staged(store: &Store, key: &str) -> (std::path::PathBuf, Value) {
        let out = store.partial(key);
        fs::create_dir_all(out.join("content/entity")).unwrap();
        let value = json!({"kind":"entity","key":"Q1","status":"present","identity":"Q1","revision":7,"entity":{"id":"Q1","lastrevid":7}});
        let data = serde_json::to_vec(&value).unwrap();
        let digest = store::sha256_hex(&data);
        let path = format!("content/entity/{digest}.json");
        fs::write(out.join(&path), data).unwrap();
        (
            out,
            json!({"schema":1,"complete":true,"failures":[],"records":[{"kind":"entity","key":"Q1","status":"present","identity":"Q1","revision":7,"checked_at":"2026-01-01T00:00:00Z","path":path,"sha256":digest}]}),
        )
    }

    #[test]
    fn pinned_shared_fetch_is_offline_and_overlapping_requests_reuse_the_fact() {
        let scratch = crate::store::tests::Scratch::new("shared-wikimedia-reuse");
        let store = Store::at(scratch.0.join("store"));
        let (out, manifest) = staged(&store, "operation-a");
        let params = vec![("content".into(), "{\"entities\":[\"Q1\"]}".into())];
        let version = date::format(date::today());
        admit(&store, &out, &manifest, &params, &version, "wikidata").unwrap();
        let source = crate::sources::embedded("wikidata");
        let snapshot =
            run(&scratch.0.join("missing-checkout"), &store, &Request { source, version: Some(version), params }, None)
                .unwrap();
        assert_eq!(snapshot.files.len(), 2);
        let pins = inputs(&store).unwrap();
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0]["key"], "Q1");
        assert!(Path::new(pins[0]["path"].as_str().unwrap()).is_absolute());
        assert_eq!(verified(&store, &pins[0]).unwrap()["revision"], 7);
        assert!(!scratch.0.join("missing-checkout").exists());
    }

    #[test]
    fn source_snapshots_keep_compact_facts_reachable_and_reject_changed_bytes() {
        let scratch = crate::store::tests::Scratch::new("shared-wikimedia-reachability");
        let store = Store::at(scratch.0.join("store"));
        let (out, manifest) = staged(&store, "operation");
        let params = vec![("content".into(), "{}".into())];
        let snapshot = admit(&store, &out, &manifest, &params, "2026-01-01", "wikidata").unwrap();
        let plan = crate::store::gc::plan(&store, &crate::store::gc::Roots::default()).unwrap();
        for file in snapshot.files {
            assert!(!plan.objects.iter().any(|(digest, _)| digest == &file.sha256));
        }
        let mut pin = inputs(&store).unwrap().remove(0);
        pin["revision"] = json!(8);
        assert!(verified(&store, &pin).unwrap_err().contains("revision"));
    }
}
