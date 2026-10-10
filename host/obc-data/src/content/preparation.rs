//! Streaming preparation and native selection of reusable content.

use super::*;

fn selector(args: &[&str], paths: &[(&str, &Path)]) -> Result<(), String> {
    let mut command = std::process::Command::new(std::env::current_exe().map_err(|e| e.to_string())?);
    command.args(args);
    for (flag, path) in paths {
        command.arg(flag).arg(path);
    }
    let status = command.stdout(std::io::stderr()).status().map_err(|e| e.to_string())?;
    if !status.success() {
        return Err(format!("native content selection failed: {status}"));
    }
    Ok(())
}

/// A detached manual preparation. Only this path permits Commons acquisition.
pub fn prepare(root: &Path, store: &Store, id: &str, request: &Request) -> Result<Value, String> {
    check_request(request)?;
    let _api = store.lock("wikimedia-api")?;
    let registry = crate::sources::Registry::load(root)?;
    let owner = crate::fetch::owner_code(
        registry.sources.iter().find(|source| source.id == "wikidata").ok_or("Wikidata source is not registered")?,
    );
    let acquisition_code = owner.code.files(root)?;
    let inputs = std::iter::once(&request.config["wikidata"])
        .chain(request.config["wikipedia"].as_object().unwrap().values())
        .chain(["wikidata_pages", "wikidata_redirects"].into_iter().filter_map(|name| request.config.get(name)))
        .chain(request.config.get("langlinks").and_then(Value::as_object).into_iter().flat_map(|sources| sources.values()))
        .filter_map(|source| source["path"].as_str())
        .chain(request.config["osm"].as_array().into_iter().flatten().filter_map(Value::as_str))
        .map(|path| {
            let metadata = fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
            Ok(json!({"path":path,"bytes":metadata.len(),"modified":metadata.modified().map_err(|e| e.to_string())?.duration_since(std::time::UNIX_EPOCH).map_err(|e| e.to_string())?.as_nanos().to_string()}))
        }).collect::<Result<Vec<_>, String>>()?;
    let producer = crate::worker::bound_code(root)?;
    let key = store::sha256_hex(
        &serde_json::to_vec(&json!({"request":request,"inputs":inputs,"code":acquisition_code,"producer":producer}))
            .map_err(|e| e.to_string())?,
    );
    let work = std::path::absolute(store.partial(&format!("content-snapshot-{key}"))).map_err(|e| e.to_string())?;
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    let config = work.join("config.json");
    store::write_atomic(&config, &serde_json::to_vec(&request.config).map_err(|e| e.to_string())?)?;
    let run = crate::engine::runs::Run::attach(store, id, &crate::engine::runs::events(store, id)?)?;
    let result = (|| {
        run.check_stop(store)?;
        python(root, &["import"], &[("--work", &work), ("--config", &config)])?;
        let roots = read(&work.join("roots.json"))?;
        let entities = roots["entities"].as_array().ok_or("invalid content roots")?;
        let links = roots["links"].as_array().ok_or("invalid content links")?;
        let peaks = roots["peaks"].as_array().into_iter().flatten().filter_map(Value::as_str).collect::<BTreeSet<_>>();
        let osm_sha256 = store::sha256_hex(&serde_json::to_vec(&roots["summits"]).map_err(|e| e.to_string())?);
        let mut summits = BTreeMap::<String, Vec<&Value>>::new();
        for summit in roots["summits"].as_array().into_iter().flatten() {
            if let Some(qid) = summit["tags"]["wikidata"].as_str() {
                summits.entry(qid.into()).or_default().push(summit);
            }
            if let Some(title) = summit["tags"]["wikipedia"].as_str() {
                summits.entry(format!("wikipedia:{title}")).or_default().push(summit);
            }
        }
        let tasks = entities
            .chunks(256)
            .map(|ids| json!({"entities":ids,"links":[],"peaks":[],"summits":[]}))
            .chain(links.chunks(256).map(|links| json!({"entities":[],"links":links,"peaks":[],"summits":[]})));
        for (group, mut scope) in tasks.enumerate() {
            run.check_stop(store)?;
            let ids = scope["entities"]
                .as_array()
                .unwrap()
                .iter()
                .chain(scope["links"].as_array().unwrap())
                .cloned()
                .collect::<Vec<_>>();
            let mut group_summits = BTreeMap::new();
            for identity in &ids {
                if peaks.contains(identity.as_str().ok_or("invalid content identity")?) {
                    scope["peaks"].as_array_mut().unwrap().push(identity.clone());
                }
                for summit in summits.get(identity.as_str().ok_or("invalid content identity")?).into_iter().flatten() {
                    group_summits.insert(summit["node_id"].as_u64().ok_or("invalid summit node")?, *summit);
                }
            }
            for summit in group_summits.into_values() {
                let qid = &summit["tags"]["wikidata"];
                scope["summits"].as_array_mut().unwrap().push(summit.clone());
                if let Some(qid) = qid.as_str() {
                    scope["links"].as_array_mut().unwrap().push(json!(format!("wikidata:{qid}")));
                }
                if let Some(title) = summit["tags"]["wikipedia"].as_str() {
                    scope["links"].as_array_mut().unwrap().push(json!(format!("wikipedia:{title}")));
                }
            }
            scope["osm_sha256"] = json!(osm_sha256);
            let batch = work.join(format!("group-{group}"));
            fs::create_dir_all(&batch).map_err(|e| e.to_string())?;
            let roots_path = batch.join("roots.json");
            fs::write(&roots_path, serde_json::to_vec(&scope).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
            let mut query = json!({"entities":scope["entities"],"links":scope["links"],"articles":[],"commons":[],"categories":[],"files":[]});
            for phase in 0..16 {
                run.check_stop(store)?;
                let out = batch.join("out");
                let manifest =
                    crate::fetch::wikimedia::archive(root, store, &batch, &work.join("catalog.sqlite"), &query, true)?;
                python(
                    root,
                    &["retain"],
                    &[("--work", &work), ("--manifest", &out.join("manifest.json")), ("--out", &out)],
                )?;
                let files = fact_files(&out, &manifest)?;
                let file_map = batch.join("files.json");
                fs::write(&file_map, serde_json::to_vec(&files).map_err(|e| e.to_string())?)
                    .map_err(|e| e.to_string())?;
                let next = batch.join("query.json");
                selector(
                    &["content-query", "--phase", &phase.to_string()],
                    &[("--files", &file_map), ("--roots", &roots_path), ("--out", &next)],
                )?;
                let next = read(&next)?;
                if phase >= 2 && next == query {
                    break;
                }
                if phase == 15 {
                    return Err("content image selection did not converge".into());
                }
                query = next;
            }
            // Successful groups stay in the source index. Temporary selection views do not.
            fs::remove_dir_all(&batch).map_err(|e| e.to_string())?;
        }
        run.check_stop(store)?;
        let out = work.join("published");
        python(root, &["export"], &[("--work", &work), ("--out", &out)])?;
        let mut manifest = read(&out.join("manifest.json"))?;
        manifest["producer"] = json!(crate::worker::bound_code(root)?);
        manifest["photo_transform_sha256"] = json!(store::sha256_hex(
            &fs::read(root.join("host/obc-pack/src/landmarks/photo.rs")).map_err(|e| e.to_string())?
        ));
        if owner.code.files(root)? != acquisition_code {
            return Err("content acquisition code changed; prepare again".into());
        }
        manifest["acquisition_code"] = json!(acquisition_code);
        fs::write(out.join("manifest.json"), serde_json::to_vec(&manifest).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let selected = admit(store, &out, &manifest)?;
        use_local(store, &selected.sha256)?;
        store::durable(&work.join("complete.json"), &serde_json::to_vec(&selected).map_err(|e| e.to_string())?)?;
        Ok(json!({"snapshot":selected.sha256,"manifest":manifest}))
    })();
    run.finish(result.as_ref().err().map(String::as_str))?;
    result
}

pub(crate) fn fact_files(out: &Path, manifest: &Value) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut files = BTreeMap::new();
    for pin in manifest["records"].as_array().ok_or("invalid fact manifest")? {
        let path = pin["path"].as_str().ok_or("fact has no path")?;
        files.insert(path.into(), out.join(path));
    }
    for asset in manifest["assets"].as_array().into_iter().flatten() {
        let digest = asset["sha256"].as_str().ok_or("asset has no digest")?;
        let path = asset["path"].as_str().ok_or("asset has no path")?;
        files.insert(format!("content/assets/{digest}"), out.join(path));
    }
    Ok(files)
}

pub(crate) fn transform_images(root: &Path, out: &Path, manifest: &mut Value) -> Result<(), String> {
    let transform =
        store::sha256_hex(&fs::read(root.join("host/obc-pack/src/landmarks/photo.rs")).map_err(|e| e.to_string())?);
    let old_assets = manifest["assets"].as_array().cloned().unwrap_or_default();
    let mut assets = BTreeMap::new();
    for pin in manifest["records"].as_array_mut().ok_or("invalid fact manifest")? {
        if pin["kind"] != "file" {
            continue;
        }
        let path = out.join(pin["path"].as_str().ok_or("file fact has no path")?);
        let mut fact = read(&path)?;
        if fact["asset"]["input"] != "rgb222" {
            let source = out.join(fact["asset"]["path"].as_str().ok_or("file has no asset path")?);
            let pixels = out.join("converted.rgb222");
            selector(&["content-photo"], &[("--input", &source), ("--out", &pixels)])?;
            let (digest, bytes) = store::hash_file(&pixels)?;
            let target = out.join("assets").join(&digest);
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::rename(&pixels, &target).map_err(|e| e.to_string())?;
            fact["source_asset"] = fact["asset"].clone();
            fact["asset"]["input"] = json!("rgb222");
            fact["asset"]["transform_sha256"] = json!(transform);
            fact["asset"]["path"] = json!(format!("assets/{digest}"));
            fact["asset"]["sha256"] = json!(digest);
            fact["asset"]["bytes"] = json!(bytes);
        }
        let asset = fact["asset"].clone();
        let bytes = serde_json::to_vec(&fact).map_err(|e| e.to_string())?;
        let digest = store::sha256_hex(&bytes);
        let relative = format!("content/file/{digest}.json");
        fs::write(out.join(&relative), bytes).map_err(|e| e.to_string())?;
        pin["path"] = json!(relative);
        pin["sha256"] = json!(digest);
        assets.insert(asset["sha256"].as_str().ok_or("converted asset has no digest")?.to_owned(), asset);
    }
    for old in old_assets {
        if !assets.contains_key(old["sha256"].as_str().unwrap_or_default()) {
            fs::remove_file(out.join(old["path"].as_str().ok_or("asset has no path")?)).map_err(|e| e.to_string())?;
        }
    }
    manifest["assets"] = json!(assets.into_values().collect::<Vec<_>>());
    Ok(())
}
