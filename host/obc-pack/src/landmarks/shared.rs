//! A temporary compiler view over the compact facts of the `wiki` source.
use super::*;

/// The facts that a view needs and the version lacks, by kind: a reader takes each as no content,
/// unlike a fact with the status `missing`, which Wikimedia confirmed.
pub type Absent = BTreeMap<&'static str, u64>;

/// A file of `files` that holds one fact.
fn fact_file(name: &str) -> bool {
    name.starts_with("content/") && !name.starts_with("content/assets/")
}

fn lacks(absent: &mut Absent, kind: &'static str) {
    *absent.entry(kind).or_default() += 1;
}

/// The counts of `absent` as text: `article 2, entity 1`.
pub fn absent_text(absent: &Absent) -> String {
    absent.iter().map(|(kind, count)| format!("{kind} {count}")).collect::<Vec<_>>().join(", ")
}

pub fn facts(files: &BTreeMap<String, std::path::PathBuf>) -> Result<BTreeMap<(String, String), Value>, String> {
    let mut facts = BTreeMap::new();
    for (name, path) in files.iter().filter(|(name, _)| fact_file(name)) {
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
    let mut landmarks = Landmarks::new();
    for fact in facts.values().filter(|fact| fact["kind"] == "entity") {
        landmarks.class(fact);
    }
    ids.iter()
        .filter(|id| facts.get(&("entity".into(), (*id).clone())).is_some_and(|fact| landmarks.selects(fact)))
        .cloned()
        .collect()
}

/// The landmark rule of the compilers over entity facts that arrive one at a time: each class
/// fact first, then each subject.
pub struct Landmarks {
    policy: policy::Policy,
    parents: BTreeMap<String, Vec<String>>,
}

impl Landmarks {
    pub fn new() -> Self {
        Landmarks { policy: policy::Policy::load(), parents: BTreeMap::new() }
    }

    /// Learn the parents of the class of an entity fact.
    pub fn class(&mut self, fact: &Value) {
        if fact["status"] != "present" {
            return;
        }
        let Some(key) = fact["key"].as_str() else { return };
        if let Ok(parents) = class_ancestors(key, &fact["entity"]) {
            self.parents.insert(key.into(), parents);
        }
    }

    /// Whether the compilers select the subject of an entity fact as a landmark: it has a category.
    pub fn selects(&self, fact: &Value) -> bool {
        let instances = entity_ids(&fact["entity"], "P31");
        self.policy.category(&instances, &self.parents).is_ok_and(|category| category.is_some())
    }
}

impl Default for Landmarks {
    fn default() -> Self {
        Self::new()
    }
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

pub fn view(files: &BTreeMap<String, std::path::PathBuf>, root: &Path, ids: &[String]) -> Result<Absent, String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let facts = facts(files)?;
    let mut absent = Absent::new();
    let mut sources = Vec::new();
    let mut paths = BTreeMap::new();
    for (name, path) in files.iter().filter(|(name, _)| fact_file(name)) {
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
        let Some(entity) = subject(&facts, id) else {
            if !facts.contains_key(&("entity".into(), id.clone())) {
                lacks(&mut absent, "entity");
            }
            continue;
        };
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
            let Some(article) = facts.get(&key) else {
                lacks(&mut absent, "article");
                continue;
            };
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
            let Some(listing) = facts.get(&key) else {
                lacks(&mut absent, "category");
                continue;
            };
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
                lacks(&mut absent, "commons");
                continue;
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
    fs::write(root.join("manifest.json"),serde_json::to_vec(&serde_json::json!({"schema":2,"sources":sources,"places":places,"aliases":aliases,"coverage":{"entity_coverage_complete":true,"asset_phase_complete":true}})).map_err(|e|e.to_string())?).map_err(|e|e.to_string())?;
    Ok(absent)
}

pub fn landmark_view(
    files: &BTreeMap<String, std::path::PathBuf>,
    root: &Path,
    ids: &[String],
) -> Result<Absent, String> {
    let facts = facts(files)?;
    let chosen = selected(&facts, ids);
    let mut absent = view(files, root, &chosen.iter().cloned().collect::<Vec<_>>())?;
    let mut omissions = Vec::new();
    for id in ids.iter().filter(|id| !chosen.contains(*id)) {
        let Some(fact) = facts.get(&("entity".into(), id.clone())) else {
            lacks(&mut absent, "entity");
            continue;
        };
        let reason =
            if fact["status"] == "missing" { "confirmed_missing_entity" } else { "policy_exclusion_or_no_category" };
        omissions.push(serde_json::json!({"qid":id,"asset":"site","reason":reason}));
    }
    let path = root.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    manifest["omissions"] = serde_json::json!(omissions);
    manifest["coverage"]["candidate_identities"] = serde_json::json!(ids.len());
    fs::write(path, serde_json::to_vec(&manifest).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(absent)
}

pub fn peak_view(files: &BTreeMap<String, std::path::PathBuf>, root: &Path) -> Result<Absent, String> {
    let source: peaks::SummitSource =
        serde_json::from_slice(&fs::read(root.join("summits.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let facts = facts(files)?;
    // The file of each link fact, by key: a summit finds its own at once. The kind is in the
    // fact, not in its file name.
    let mut links = BTreeMap::new();
    for (name, path) in files.iter().filter(|(name, _)| fact_file(name)) {
        let value: Value =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if value["kind"] == "link" {
            links.entry(string(&value, "key")?.to_owned()).or_insert_with(|| name.clone());
        }
    }
    let (mut ids, mut absent) = (BTreeSet::new(), Absent::new());
    let mut resolutions = Vec::new();
    for summit in &source.summits {
        for kind in ["wikidata", "wikipedia"] {
            let Some(tag) = summit.tags.get(kind) else { continue };
            let key = format!("{kind}:{tag}");
            let Some(fact) = facts.get(&("link".into(), key.clone())) else {
                lacks(&mut absent, "link");
                continue;
            };
            let path = links.get(&key).cloned().ok_or("missing link file")?;
            if fact["status"] == "present" {
                ids.insert(string(fact, "identity")?.to_owned());
            }
            resolutions.push(serde_json::json!({"node_id":summit.node_id,"kind":kind,"path":path,"status":if fact["status"] == "present" {"resolved"} else {"confirmed_missing"}}));
        }
    }
    for (kind, count) in view(files, root, &ids.into_iter().collect::<Vec<_>>())? {
        *absent.entry(kind).or_default() += count;
    }
    let manifest_path = root.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_slice(&fs::read(&manifest_path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    let data = fs::read(root.join("summits.json")).map_err(|e| e.to_string())?;
    manifest["sources"].as_array_mut().unwrap().push(
        serde_json::json!({"path":"summits.json","sha256":hash(&data),"bytes":data.len(),"url":"osm:regional-summits"}),
    );
    manifest["peaks"] = serde_json::json!({"summits_path":"summits.json","resolutions":resolutions});
    fs::write(manifest_path, serde_json::to_vec(&manifest).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
    Ok(absent)
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
            json!({"kind":"entity","key":"Q9","identity":"Q1","revision":7,"status":"present","entity":{"id":"Q1","lastrevid":7,"redirects":{"from":"Q9","to":"Q1"},"labels":{"en":{"value":"Castle"}},"sitelinks":{"enwiki":{"title":"Old Castle"}},"claims":{"P31":[{"mainsnak":{"datavalue":{"value":{"id":"Q23413"}}}}],"P373":[{"mainsnak":{"datavalue":{"value":"Castle"}}}],"P625":[{"mainsnak":{"datavalue":{"value":{"latitude":0.0,"longitude":0.0,"globe":"http://www.wikidata.org/entity/Q2"}}}}]}}}),
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
            json!({"kind":"category","key":"Category:Castle","identity":"Category:Castle","status":"present","members":[],"bounded":true,"limit":photo_policy()["fallback"]["files"],"limit_reached":false,"truncated":false,"continuation":{}}),
        );
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
        let mismatched = root.join("mismatched-view");
        for (index, qid) in [serde_json::json!("Q3"), Value::Null].into_iter().enumerate() {
            let mut unrelated = facts(&files).unwrap()[&("article".into(), "en:Old Castle".into())].clone();
            files.retain(|_, path| {
                let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                value["kind"] != "article"
            });
            unrelated["qid"] = qid;
            pin(&root, &mut files, unrelated);
            landmark_view(&files, &mismatched, &["Q9".into()]).unwrap();
            let rejected =
                compile(&mismatched.join("manifest.json"), &boundary, &root.join(format!("rejected-{index}")), false)
                    .unwrap();
            assert!(rejected.records.is_empty());
            assert!(rejected.wikipedia_aliases.is_empty());
            assert!(rejected.omissions.iter().any(|omission| {
                omission.qid == "Q1"
                    && omission.asset == "article"
                    && omission.reason == "en: article_identity_mismatch"
            }));
        }
        let manifest: Value = serde_json::from_slice(&fs::read(mismatched.join("manifest.json")).unwrap()).unwrap();
        let sources: Vec<Source> = serde_json::from_value(manifest["sources"].clone()).unwrap();
        let synthetic = serde_json::json!({"id":"wiki-en-4","sitelinks":{"enwiki":{"title":"Old Castle"}}});
        assert!(article(&mismatched, &sources, &synthetic, &manifest["places"][0]["articles"][0]).is_ok());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_fact_that_the_version_lacks_is_no_content_and_is_counted() {
        let root = obcm_testkit::scratch::scratch_dir("landmarks", "shared-absent");
        let mut files = BTreeMap::new();
        let claim = |value: Value| json!([{"mainsnak": {"datavalue": {"value": value}}}]);
        let claims = json!({"P31": claim(json!({"id": "Q23413"})), "P373": claim(json!("Castle")),
                            "P18": claim(json!("Hill.jpg"))});
        let castle = json!({"id": "Q1", "sitelinks": {"enwiki": {"title": "Castle"}}, "claims": claims});
        pin(&root, &mut files, json!({"kind":"entity","key":"Q1","identity":"Q1","status":"present","entity":castle}));
        let class = json!({"id": "Q23413", "claims": {}});
        pin(
            &root,
            &mut files,
            json!({"kind":"entity","key":"Q23413","identity":"Q23413","status":"present","entity":class}),
        );
        // Q7 is newer than the version; the castle has no article, category or Commons fact.
        let absent = landmark_view(&files, &root.join("landmarks"), &["Q1".into(), "Q7".into()]).unwrap();
        assert_eq!(absent, Absent::from([("article", 1), ("category", 1), ("commons", 1), ("entity", 1)]));

        let view = root.join("peaks");
        fs::create_dir_all(&view).unwrap();
        let tags = [("natural", "peak"), ("name", "Testhorn"), ("wikidata", "Q8")];
        let tags = tags.map(|(key, value)| (key.to_string(), value.to_string())).into();
        let summit = peaks::Summit { node_id: 1, latitude: 0.0, longitude: 0.0, tags };
        let source = peaks::SummitSource { schema: 1, osm_sha256: "0".repeat(64), summits: vec![summit] };
        fs::write(view.join("summits.json"), serde_json::to_vec(&source).unwrap()).unwrap();
        assert_eq!(peak_view(&files, &view).unwrap(), Absent::from([("link", 1)]));
        let boundary = root.join("boundary.json");
        fs::write(&boundary, r#"{"type":"Polygon","coordinates":[[[-1,-1],[1,-1],[1,1],[-1,1],[-1,-1]]]}"#).unwrap();
        let peaks = peaks::compile(&view.join("manifest.json"), &boundary, &root.join("compiled"), false).unwrap();
        assert!(peaks.records.is_empty(), "the summit stays an OSM summit");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_summit_finds_its_link_fact_by_the_kind_in_the_fact_and_not_by_the_file_name() {
        let root = obcm_testkit::scratch::scratch_dir("landmarks", "shared-peak-links");
        let mut files = BTreeMap::new();
        let entity =
            json!({"kind":"entity","key":"Q5","identity":"Q5","revision":1,"status":"present","entity":{"id":"Q5"}});
        pin(&root, &mut files, entity);
        let missing = pin(&root, &mut files, json!({"kind":"link","key":"wikipedia:en:Nowhere","status":"missing"}));
        // A capture names a fact by its digest alone.
        let link = json!({"kind":"link","key":"wikidata:Q5","identity":"Q5","status":"present"});
        let link = serde_json::to_vec(&link).unwrap();
        let name = format!("content/fact/{}.json", hash(&link));
        fs::write(root.join("fact.json"), &link).unwrap();
        files.insert(name.clone(), root.join("fact.json"));
        let view = root.join("view");
        fs::create_dir_all(&view).unwrap();
        let tags = [("natural", "peak"), ("name", "Testhorn"), ("wikidata", "Q5"), ("wikipedia", "en:Nowhere")];
        let tags = tags.map(|(key, value)| (key.to_string(), value.to_string())).into();
        let summit = peaks::Summit { node_id: 1, latitude: 0.0, longitude: 0.0, tags };
        let source = peaks::SummitSource { schema: 1, osm_sha256: "0".repeat(64), summits: vec![summit] };
        fs::write(view.join("summits.json"), serde_json::to_vec(&source).unwrap()).unwrap();
        peak_view(&files, &view).unwrap();
        let manifest: Value = serde_json::from_slice(&fs::read(view.join("manifest.json")).unwrap()).unwrap();
        let resolutions = json!([
            {"node_id": 1, "kind": "wikidata", "path": name, "status": "resolved"},
            {"node_id": 1, "kind": "wikipedia", "path": missing["path"], "status": "confirmed_missing"},
        ]);
        assert_eq!(manifest["peaks"]["resolutions"], resolutions);
        fs::remove_dir_all(root).unwrap();
    }
}
