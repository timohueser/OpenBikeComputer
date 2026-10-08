//! A temporary compiler view over compact source pins; no response replica enters the Store.
use super::*;

pub fn facts(files: &BTreeMap<String, std::path::PathBuf>) -> Result<BTreeMap<(String, String), Value>, String> {
    let mut facts = BTreeMap::new();
    for (name, path) in files.iter().filter(|(name, _)| {
        name.starts_with("content/") && !name.starts_with("content/manifest/") && !name.starts_with("content/assets/")
    }) {
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        if Path::new(name).file_stem().and_then(|name| name.to_str()) != Some(hash(&bytes).as_str()) {
            return Err(format!("shared fact checksum changed: {name}"));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{name}: {e}"))?;
        if !matches!(value["status"].as_str(), Some("present" | "missing")) {
            return Err("unresolved shared content".into());
        }
        let key = (string(&value, "kind")?.to_owned(), string(&value, "key")?.to_owned());
        if facts.insert(key, value).is_some() {
            return Err("conflicting shared source facts".into());
        }
    }
    Ok(facts)
}

pub fn selected(facts: &BTreeMap<(String, String), Value>, ids: &[String]) -> BTreeSet<String> {
    let parents = facts
        .values()
        .filter(|fact| fact["kind"] == "entity" && fact["status"] == "present")
        .filter_map(|fact| {
            class_ancestors(fact["key"].as_str()?, &fact["entity"])
                .ok()
                .map(|parents| (fact["key"].as_str().unwrap().into(), parents))
        })
        .collect();
    let policy = policy::Policy::load();
    ids.iter()
        .filter(|id| {
            facts.get(&("entity".into(), (*id).clone())).is_some_and(|fact| {
                policy.category(&entity_ids(&fact["entity"], "P31"), &parents).is_ok_and(|category| category.is_some())
            })
        })
        .cloned()
        .collect()
}

pub fn subject(facts: &BTreeMap<(String, String), Value>, id: &str) -> Option<Value> {
    if let Some(fact) = facts
        .get(&("entity".into(), id.into()))
        .or_else(|| facts.values().find(|fact| fact["kind"] == "entity" && fact["identity"] == id))
    {
        return (fact["status"] == "present").then(|| fact["entity"].clone());
    }
    let link =
        facts.values().find(|fact| fact["kind"] == "link" && fact["identity"] == id && fact["status"] == "present")?;
    let labels = link["sitelinks"]
        .as_object()?
        .iter()
        .filter_map(|(wiki, link)| {
            wiki.strip_suffix("wiki").map(|language| (language.to_owned(), serde_json::json!({"value":link["title"]})))
        })
        .collect::<serde_json::Map<String, Value>>();
    Some(serde_json::json!({"id":id,"sitelinks":link["sitelinks"],"labels":labels}))
}

pub fn categories(entity: &Value) -> Vec<String> {
    let policy: Value = serde_json::from_slice(PHOTO_POOL_BYTES).expect("checked photo policy");
    let limit = policy["fallback"]["categories"].as_u64().unwrap_or(0) as usize;
    claims(entity, "P373")
        .filter_map(|claim| claim["mainsnak"]["datavalue"]["value"].as_str())
        .map(|name| format!("Category:{}", name.replace('_', " ")))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .take(limit)
        .collect()
}

pub fn lead_image(fact: &Value) -> Option<String> {
    assets::commons_lead_image(&scraper::Html::parse_fragment(fact["lead_html"].as_str()?))
}

pub fn view(files: &BTreeMap<String, std::path::PathBuf>, root: &Path, ids: &[String]) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let facts = facts(files)?;
    let mut sources = Vec::new();
    let mut paths = BTreeMap::new();
    for (name, path) in files.iter().filter(|(name, _)| {
        name.starts_with("content/") && !name.starts_with("content/manifest/") && !name.starts_with("content/assets/")
    }) {
        let value: Value =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        let relative = match value["kind"].as_str() {
            Some("entity") => format!("classes/{}.json", string(&value, "key")?),
            _ => name.clone(),
        };
        let target = root.join(&relative);
        fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(path, &target).map_err(|e| e.to_string())?;
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        if value["kind"] == "entity" {
            let locale_path = format!("locales/{}.json", string(&value, "key")?);
            let target = root.join(&locale_path);
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(path, &target).map_err(|e| e.to_string())?;
            sources.push(serde_json::json!({"path":locale_path,"sha256":hash(&bytes),"bytes":bytes.len(),"url":"wikimedia:locale"}));
            let alias = format!("entities/{}.json", string(&value, "key")?);
            let target = root.join(&alias);
            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
            fs::copy(path, &target).map_err(|e| e.to_string())?;
            sources.push(
                serde_json::json!({"path":alias,"sha256":hash(&bytes),"bytes":bytes.len(),"url":"wikimedia:entity"}),
            );
            let canonical = value["identity"].as_str().unwrap_or(string(&value, "key")?);
            if canonical != string(&value, "key")? && !facts.contains_key(&("entity".into(), canonical.into())) {
                let alias = format!("entities/{canonical}.json");
                if !sources.iter().any(|source| source["path"] == alias) {
                    fs::copy(path, root.join(&alias)).map_err(|e| e.to_string())?;
                    sources.push(serde_json::json!({"path":alias,"sha256":hash(&bytes),"bytes":bytes.len(),"url":"wikimedia:entity"}));
                }
            }
        }
        sources.push(serde_json::json!({"path":relative,"sha256":hash(&bytes),"bytes":bytes.len(),"url":value["url"].as_str().unwrap_or("wikimedia:content")}));
        paths.insert((string(&value, "kind")?.to_owned(), string(&value, "key")?.to_owned()), relative);
    }
    for (name, path) in files.iter().filter(|(name, _)| name.starts_with("content/assets/")) {
        let target = root.join(name);
        fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        fs::copy(path, &target).map_err(|e| e.to_string())?;
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let file = facts
            .values()
            .find(|fact| fact["kind"] == "file" && fact["asset"]["sha256"] == hash(&bytes))
            .ok_or("unregistered shared media asset")?;
        sources.push(
            serde_json::json!({"path":name,"sha256":hash(&bytes),"bytes":bytes.len(),"url":file["asset"]["url"]}),
        );
    }
    let mut places = Vec::new();
    let mut aliases = BTreeMap::new();
    for id in ids {
        let entity = subject(&facts, id).ok_or_else(|| format!("missing shared subject: {id}"))?;
        let canonical = string(&entity, "id")?.to_owned();
        if canonical != *id {
            aliases.insert(id.clone(), canonical.clone());
        }
        let fact = serde_json::json!({"entity":entity});
        let mut articles = Vec::new();
        let mut images = Vec::new();
        for (language, _) in locale::languages() {
            let Some(title) = fact["entity"]["sitelinks"][format!("{language}wiki")]["title"].as_str() else {
                continue;
            };
            let key = ("article".into(), format!("{language}:{title}"));
            let Some(article) = facts.get(&key) else { return Err(format!("missing shared article: {}", key.1)) };
            if article["status"] == "present" {
                articles.push(serde_json::json!({"language":language,"title":article["title"],"path":paths[&key],"revision":article["revision"],"url":article["url"],"compact":true,"requested_title":title,"aliases":article["aliases"]}));
            }
        }
        let mut candidates = Vec::new();
        for article in &articles {
            let language = article["language"].as_str().unwrap();
            let key = (
                "article".into(),
                format!(
                    "{language}:{}",
                    fact["entity"]["sitelinks"][format!("{language}wiki")]["title"].as_str().unwrap()
                ),
            );
            if let Some(filename) = facts.get(&key).and_then(lead_image) {
                candidates.push((filename, "wikipedia-lead", article["language"].clone()));
            }
        }
        for filename in
            claims(&fact["entity"], "P18").filter_map(|claim| claim["mainsnak"]["datavalue"]["value"].as_str())
        {
            candidates.push((filename.into(), "P18", Value::Null));
        }
        let mut listings = Vec::new();
        for category in categories(&fact["entity"]) {
            let key = ("category".into(), category.clone());
            let listing = facts.get(&key).ok_or_else(|| format!("missing bounded category fact: {category}"))?;
            if listing["status"] != "present" {
                continue;
            }
            listings.push(serde_json::json!({"title":category,"complete":!listing["truncated"].as_bool().unwrap_or(false),"limit":listing["limit"],"pages":[{"path":paths[&key],"continuation":null}]}));
            for member in listing["members"].as_array().into_iter().flatten() {
                if let Some(filename) = member["title"].as_str().and_then(|title| title.strip_prefix("File:")) {
                    candidates.push((filename.into(), "commons-category", Value::Null));
                }
            }
        }
        for (filename, source, language) in candidates {
            let key = ("commons".into(), format!("File:{}", filename.replace('_', " ")));
            let Some(metadata) = facts.get(&key) else {
                return Err(format!("missing shared Commons metadata: {}", key.1));
            };
            if metadata["status"] == "present" {
                let mut image = serde_json::json!({"filename":filename,"metadata_path":paths[&key],"source":source,"language":language});
                if let Some(path) = paths.get(&("mediainfo".into(), format!("M{}", metadata["pageid"]))) {
                    image["depicts_path"] = serde_json::json!(path);
                }
                if let Some(file) = facts.get(&("file".into(), key.1.clone())) {
                    image["path"] = serde_json::json!(format!("content/assets/{}", string(&file["asset"], "sha256")?));
                    if file["asset"]["input"] == "thumbnail500" {
                        for witness in ["revision_before", "revision_after"] {
                            let data = serde_json::to_vec(&file[witness]).map_err(|e| e.to_string())?;
                            if file[witness].is_null() {
                                return Err("thumbnail has no revision witness".into());
                            }
                            let path = format!("proofs/{}.json", hash(&data));
                            let target = root.join(&path);
                            fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
                            fs::write(target, &data).map_err(|e| e.to_string())?;
                            if !sources.iter().any(|source| source["path"] == path) {
                                sources.push(serde_json::json!({"path":path,"sha256":hash(&data),"bytes":data.len(),"url":"https://commons.wikimedia.org/w/api.php"}));
                            }
                            image[format!("{witness}_path")] = serde_json::json!(path);
                        }
                    }
                }
                images.push(image);
            }
        }
        places.push(serde_json::json!({"qid":canonical,"entity_path":paths.get(&("entity".into(),id.clone())).or_else(||paths.get(&("entity".into(),canonical.clone()))),"articles":articles,"images":images,"commons_categories":listings}));
    }
    fs::write(root.join("manifest.json"),serde_json::to_vec(&serde_json::json!({"schema":2,"sources":sources,"places":places,"aliases":aliases,"coverage":{"entity_coverage_complete":true,"asset_phase_complete":true}})).map_err(|e|e.to_string())?).map_err(|e|e.to_string())
}

pub fn landmark_view(files: &BTreeMap<String, std::path::PathBuf>, root: &Path, ids: &[String]) -> Result<(), String> {
    let facts = facts(files)?;
    let chosen = selected(&facts, ids);
    view(files, root, &chosen.iter().cloned().collect::<Vec<_>>())?;
    let mut omissions = Vec::new();
    for id in ids.iter().filter(|id| !chosen.contains(*id)) {
        let fact = facts.get(&("entity".into(), id.clone())).ok_or_else(|| format!("missing shared entity: {id}"))?;
        let reason =
            if fact["status"] == "missing" { "confirmed_missing_entity" } else { "policy_exclusion_or_no_category" };
        omissions.push(serde_json::json!({"qid":id,"asset":"site","reason":reason}));
    }
    let path = root.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    manifest["omissions"] = serde_json::json!(omissions);
    manifest["coverage"]["candidate_identities"] = serde_json::json!(ids.len());
    fs::write(path, serde_json::to_vec(&manifest).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

pub fn peak_view(files: &BTreeMap<String, std::path::PathBuf>, root: &Path) -> Result<(), String> {
    let source: peaks::SummitSource =
        serde_json::from_slice(&fs::read(root.join("summits.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let facts = facts(files)?;
    let mut ids = BTreeSet::new();
    let mut resolutions = Vec::new();
    for summit in &source.summits {
        for kind in ["wikidata", "wikipedia"] {
            let Some(tag) = summit.tags.get(kind) else { continue };
            let key = format!("{kind}:{tag}");
            let Some(fact) = facts.get(&("link".into(), key.clone())) else {
                return Err(format!("missing link fact: {key}"));
            };
            let path = files
                .iter()
                .find_map(|(name, path)| {
                    let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
                    (value["kind"] == "link" && value["key"] == key).then_some(name.clone())
                })
                .ok_or("missing link file")?;
            if fact["status"] == "present" {
                ids.insert(string(fact, "identity")?.to_owned());
            }
            resolutions.push(serde_json::json!({"node_id":summit.node_id,"kind":kind,"path":path,"status":if fact["status"] == "present" {"resolved"} else {"confirmed_missing"}}));
        }
    }
    view(files, root, &ids.into_iter().collect::<Vec<_>>())?;
    let manifest_path = root.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let data = fs::read(root.join("summits.json")).map_err(|e| e.to_string())?;
    manifest["sources"].as_array_mut().unwrap().push(
        serde_json::json!({"path":"summits.json","sha256":hash(&data),"bytes":data.len(),"url":"osm:regional-summits"}),
    );
    manifest["peaks"] = serde_json::json!({"summits_path":"summits.json","resolutions":resolutions});
    fs::write(manifest_path, serde_json::to_vec(&manifest).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

/// Publish compact facts and their pins; conversion pixels stay in source snapshots locally.
pub fn bundles(files: &BTreeMap<String, std::path::PathBuf>, output: &Path) -> Result<(), String> {
    let mut entries = BTreeMap::new();
    for (_, path) in files.iter().filter(|(name, _)| name.starts_with("content/manifest/")) {
        let manifest: Value =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        for pin in manifest["records"].as_array().ok_or("shared manifest records missing")? {
            if pin["kind"] == "file" {
                continue;
            }
            let kind = string(pin, "kind")?;
            let digest = string(pin, "sha256")?;
            let name = format!("content/{kind}/{digest}.json");
            let path = files.get(&name).ok_or("bundle fact is not in the compile inputs")?;
            let data = fs::read_to_string(path).map_err(|e| e.to_string())?;
            if hash(data.as_bytes()) != digest {
                return Err("bundle fact digest changed".into());
            }
            let mut pin = pin.clone();
            pin.as_object_mut().unwrap().remove("path");
            entries
                .insert((kind.to_owned(), string(&pin, "key")?.to_owned()), serde_json::json!({"pin":pin,"data":data}));
        }
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    if entries.is_empty() {
        return Ok(());
    }
    let mut records = Vec::new();
    let mut bytes = 64usize;
    let mut index = 0;
    for entry in entries.into_values() {
        let size = serde_json::to_vec(&entry).map_err(|e| e.to_string())?.len() + 1;
        if size + 64 > MAX_JSON_SOURCE as usize {
            return Err("compact fact exceeds publication bundle limit".into());
        }
        if bytes + size > MAX_JSON_SOURCE as usize {
            write_bundle(output, index, &records)?;
            index += 1;
            records.clear();
            bytes = 64;
        }
        records.push(entry);
        bytes += size;
    }
    write_bundle(output, index, &records)
}

fn write_bundle(output: &Path, index: usize, records: &[Value]) -> Result<(), String> {
    fs::write(
        output.join(format!("{index:04}.json")),
        serde_json::to_vec(&serde_json::json!({"schema":1,"records":records})).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn pin(root: &Path, files: &mut BTreeMap<String, std::path::PathBuf>, value: Value) -> Value {
        let data = serde_json::to_vec(&value).unwrap();
        let digest = hash(&data);
        let name = format!("content/{}/{digest}.json", value["kind"].as_str().unwrap());
        let path = root.join(&name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, data).unwrap();
        files.insert(name.clone(), path);
        json!({"kind":value["kind"],"key":value["key"],"identity":value["identity"],"revision":value["revision"],"status":value["status"],"sha256":digest,"checked_at":"2026-01-01T00:00:00Z","path":name})
    }

    #[test]
    fn compact_facts_compile_offline_with_exact_aliases_and_omission_reasons() {
        let root = obcm_testkit::scratch::scratch_dir("landmarks", "shared-offline");
        let mut files = BTreeMap::new();
        pin(
            &root,
            &mut files,
            json!({"kind":"entity","key":"Q9","identity":"Q1","revision":7,"status":"present","entity":{"id":"Q1","lastrevid":7,"redirects":{"from":"Q9","to":"Q1"},"labels":{"en":{"value":"Castle"}},"sitelinks":{"enwiki":{"title":"Old Castle"}},"claims":{"P31":[{"mainsnak":{"datavalue":{"value":{"id":"Q23413"}}}}],"P625":[{"mainsnak":{"datavalue":{"value":{"latitude":0.0,"longitude":0.0,"globe":"http://www.wikidata.org/entity/Q2"}}}}]}}}),
        );
        pin(
            &root,
            &mut files,
            json!({"kind":"entity","key":"Q23413","identity":"Q23413","revision":2,"status":"present","entity":{"id":"Q23413","claims":{}}}),
        );
        pin(&root, &mut files, json!({"kind":"entity","key":"Q2","status":"missing"}));
        pin(
            &root,
            &mut files,
            json!({"kind":"article","key":"en:Old Castle","identity":"en:4","revision":9,"status":"present","language":"en","title":"Castle","qid":"Q1","aliases":[{"from":"Old Castle","to":"Castle"}],"url":"https://en.wikipedia.org/w/index.php?title=Castle&oldid=9","license":{"url":"https://creativecommons.org/licenses/by-sa/4.0/"},"original_notices":"","lead_html":"<section><p>Castle is a fortified building with thick stone walls. It stands beside a river.</p></section>"}),
        );
        let view_root = root.join("view");
        landmark_view(&files, &view_root, &["Q9".into(), "Q2".into()]).unwrap();
        let boundary = root.join("boundary.json");
        fs::write(&boundary, r#"{"type":"Polygon","coordinates":[[[-1,-1],[1,-1],[1,1],[-1,1],[-1,-1]]]}"#).unwrap();
        let compiled = compile(&view_root.join("manifest.json"), &boundary, &root.join("compiled"), false).unwrap();
        assert_eq!(compiled.records.len(), 1, "{:?}", compiled.omissions);
        assert_eq!(compiled.records[0].qid, "Q1");
        assert_eq!(compiled.aliases["Q9"], "Q1");
        assert_eq!(compiled.wikipedia_aliases["en:Old Castle"], "Q1");
        assert_eq!(compiled.wikipedia_aliases["en:Castle"], "Q1");
        assert!(compiled
            .omissions
            .iter()
            .any(|omission| omission.qid == "Q2" && omission.reason == "confirmed_missing_entity"));
        assert!(compiled.records[0].photo.is_none());
        let mut unrelated = facts(&files).unwrap()[&("article".into(), "en:Old Castle".into())].clone();
        files.retain(|_, path| {
            let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            value["kind"] != "article"
        });
        unrelated["qid"] = "Q3".into();
        pin(&root, &mut files, unrelated);
        let mismatched = root.join("mismatched-view");
        landmark_view(&files, &mismatched, &["Q9".into()]).unwrap();
        let rejected = compile(&mismatched.join("manifest.json"), &boundary, &root.join("rejected"), false).unwrap();
        assert!(rejected.records.is_empty());
        assert!(rejected.wikipedia_aliases.is_empty());
        assert!(rejected.omissions.iter().any(|omission| {
            omission.qid == "Q1" && omission.asset == "article" && omission.reason == "en: article_identity_mismatch"
        }));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn source_publication_has_compact_provenance_and_excludes_conversion_inputs() {
        let root = obcm_testkit::scratch::scratch_dir("landmarks", "shared-bundle");
        let mut files = BTreeMap::new();
        let entity = pin(
            &root,
            &mut files,
            json!({"kind":"entity","key":"Q1","identity":"Q1","revision":7,"status":"present","entity":{"id":"Q1"}}),
        );
        let pixels = pin(
            &root,
            &mut files,
            json!({"kind":"file","key":"File:Image.jpg","status":"present","asset":{"path":"assets/private.jpg","sha256":"pixels"}}),
        );
        let manifest = root.join("manifest.json");
        fs::write(&manifest, serde_json::to_vec(&json!({"schema":1,"records":[entity.clone(),pixels]})).unwrap())
            .unwrap();
        files.insert(format!("content/manifest/{}.json", hash(&fs::read(&manifest).unwrap())), manifest);
        let output = root.join("bundles");
        bundles(&files, &output).unwrap();
        let data = fs::read(output.join("0000.json")).unwrap();
        let bundle: Value = serde_json::from_slice(&data).unwrap();
        assert_eq!(bundle["records"].as_array().unwrap().len(), 1);
        let record = &bundle["records"][0];
        assert_eq!(hash(record["data"].as_str().unwrap().as_bytes()), entity["sha256"]);
        assert_eq!(record["pin"]["revision"], 7);
        assert!(!String::from_utf8(data).unwrap().contains("private.jpg"));
        for source in ["wikidata", "wikipedia", "commons"] {
            assert!(!obc_data::sources::embedded(source).r2_copy);
        }
        fs::remove_dir_all(root).unwrap();
    }
}
