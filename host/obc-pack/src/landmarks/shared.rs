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

/// One fact file of a `wiki` export: its fact, and what a view looks up without reading it.
struct Entry {
    kind: String,
    key: String,
    present: bool,
    identity: Option<String>,
    url: Option<String>,
    /// The SHA-256 of the asset of a `file` fact.
    asset: Option<String>,
}

/// The facts of a `wiki` export, by kind and key. A region holds about a million, so [`facts`]
/// reads each file once to index it and keeps no fact: [`Facts::get`] reads one from its file.
pub struct Facts<'a> {
    files: &'a BTreeMap<String, std::path::PathBuf>,
    /// Each fact file, by name.
    entries: BTreeMap<String, Entry>,
    /// The name of each fact, by kind and key.
    names: BTreeMap<(String, String), String>,
    /// The key of the first entity fact of each identity, and of the first present link fact.
    identities: BTreeMap<(String, String), String>,
    /// The key of the first `file` fact of each asset SHA-256.
    assets: BTreeMap<String, String>,
}

impl Facts<'_> {
    /// The fact with `key`, read from its file.
    pub fn get(&self, key: &(String, String)) -> Option<Value> {
        let path = self.files.get(self.names.get(key)?)?;
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }

    pub fn contains_key(&self, key: &(String, String)) -> bool {
        self.names.contains_key(key)
    }

    /// The key of the fact of `kind` whose identity is `identity`.
    fn identity(&self, kind: &str, identity: &str) -> Option<(String, String)> {
        let key = self.identities.get(&(kind.to_owned(), identity.to_owned()))?;
        Some((kind.to_owned(), key.clone()))
    }
}

pub fn facts(files: &BTreeMap<String, std::path::PathBuf>) -> Result<Facts<'_>, String> {
    let mut facts = Facts {
        files,
        entries: BTreeMap::new(),
        names: BTreeMap::new(),
        identities: BTreeMap::new(),
        assets: BTreeMap::new(),
    };
    for (name, path) in files.iter().filter(|(name, _)| fact_file(name)) {
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        if Path::new(name).file_stem().and_then(|name| name.to_str()) != Some(hash(&bytes).as_str()) {
            return Err(format!("shared fact checksum changed: {name}"));
        }
        let value: Value = serde_json::from_slice(&bytes).map_err(|e| format!("{name}: {e}"))?;
        if !matches!(value["status"].as_str(), Some("present" | "missing")) {
            return Err("unresolved shared content".into());
        }
        let (kind, key) = (string(&value, "kind")?.to_owned(), string(&value, "key")?.to_owned());
        if facts.names.insert((kind.clone(), key.clone()), name.clone()).is_some() {
            return Err("conflicting shared source facts".into());
        }
        let text = |field: &str| value[field].as_str().map(String::from);
        let asset = value["asset"]["sha256"].as_str().filter(|_| kind == "file").map(String::from);
        let present = value["status"] == "present";
        let entry = Entry { present, identity: text("identity"), url: text("url"), asset, kind, key };
        facts.entries.insert(name.clone(), entry);
    }
    // In the order of kind and key, as a scan of the facts finds them.
    for ((kind, key), name) in &facts.names {
        let entry = &facts.entries[name];
        let indexed = kind == "entity" || (kind == "link" && entry.present);
        if let Some(identity) = entry.identity.as_ref().filter(|_| indexed) {
            facts.identities.entry((kind.clone(), identity.clone())).or_insert_with(|| key.clone());
        }
        if let Some(asset) = &entry.asset {
            facts.assets.entry(asset.clone()).or_insert_with(|| key.clone());
        }
    }
    Ok(facts)
}

pub fn selected(facts: &Facts, ids: &[String]) -> BTreeSet<String> {
    let mut landmarks = Landmarks::new();
    for key in facts.names.keys().filter(|(kind, _)| kind == "entity") {
        if let Some(fact) = facts.get(key) {
            landmarks.class(&fact);
        }
    }
    ids.iter()
        .filter(|id| facts.get(&("entity".into(), (*id).clone())).is_some_and(|fact| landmarks.selects(&fact)))
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

pub fn subject(facts: &Facts, id: &str) -> Option<Value> {
    let entity = facts.get(&("entity".into(), id.into())).or_else(|| facts.get(&facts.identity("entity", id)?));
    if let Some(fact) = entity {
        return (fact["status"] == "present").then(|| fact["entity"].clone());
    }
    let link = facts.get(&facts.identity("link", id)?)?;
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
    let (absent, view) = view_of(&facts(files)?, root, ids, &BTreeSet::new())?;
    view.write(root)?;
    Ok(absent)
}

/// Link `path` to `target`, or copy it where the file system cannot link. An older `target` goes
/// first: it can be a link, and a copy onto a link writes the file that it links.
fn place(path: &Path, target: &Path) -> Result<(), String> {
    fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
    match fs::remove_file(target) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!("{}: {error}", target.display()))
        }
        _ => {}
    }
    if fs::hard_link(path, target).is_err() {
        fs::copy(path, target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    Ok(())
}

#[derive(Serialize)]
struct ViewSource {
    path: String,
    sha256: String,
    bytes: u64,
    url: String,
}

/// The compiler input of a view, which [`View::write`] writes once as `manifest.json`.
struct View {
    sources: Vec<ViewSource>,
    places: Vec<Value>,
    aliases: BTreeMap<String, String>,
    omissions: Vec<Value>,
    coverage: serde_json::Map<String, Value>,
    peaks: Option<Value>,
}

impl View {
    fn write(&self, root: &Path) -> Result<(), String> {
        #[derive(Serialize)]
        struct Manifest<'a> {
            schema: u32,
            sources: &'a [ViewSource],
            places: &'a [Value],
            aliases: &'a BTreeMap<String, String>,
            #[serde(skip_serializing_if = "<[Value]>::is_empty")]
            omissions: &'a [Value],
            coverage: &'a serde_json::Map<String, Value>,
            #[serde(skip_serializing_if = "Option::is_none")]
            peaks: Option<&'a Value>,
        }
        let manifest = Manifest {
            schema: 2,
            sources: &self.sources,
            places: &self.places,
            aliases: &self.aliases,
            omissions: &self.omissions,
            coverage: &self.coverage,
            peaks: self.peaks.as_ref(),
        };
        let path = root.join("manifest.json");
        let mut file =
            std::io::BufWriter::new(fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?);
        serde_json::to_writer(&mut file, &manifest).map_err(|e| e.to_string())?;
        std::io::Write::flush(&mut file).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// What a view holds: the facts that its places name, and the closures of classes and locales
/// that the compilers read for them. A region has about a million facts, and its places need a
/// few thousand.
#[derive(Default)]
struct Needed {
    /// Facts by kind and key. An entity here is in `classes/`.
    facts: BTreeSet<(String, String)>,
    /// The P31 types of the places, whose P279 closure joins `facts`.
    classes: BTreeSet<String>,
    /// The P131 and P17 places of the places, whose P131 closure is in `locales/`.
    locales: BTreeSet<String>,
    /// The places and their canonical items, in `entities/`.
    entities: BTreeSet<String>,
    /// The SHA-256 of each asset of a `file` fact that a place names.
    assets: BTreeSet<String>,
}

impl Needed {
    /// The path in the view of the fact with `key`, which the view then holds.
    fn path(&mut self, facts: &Facts, key: &(String, String)) -> Option<String> {
        let name = facts.names.get(key)?;
        self.facts.insert(key.clone());
        Some(match key.0.as_str() {
            "entity" => format!("classes/{}.json", key.1),
            _ => name.clone(),
        })
    }

    /// Add the P279 closure of `classes` and the P131 closure of `locales`.
    fn close(&mut self, facts: &Facts) {
        let (mut pending, mut visited): (Vec<String>, BTreeSet<String>) =
            (self.classes.iter().cloned().collect(), BTreeSet::new());
        while let Some(id) = pending.pop() {
            let key = ("entity".to_owned(), id.clone());
            if !visited.insert(id.clone()) || !facts.contains_key(&key) {
                continue;
            }
            self.facts.insert(key.clone());
            if let Some(fact) = facts.get(&key).filter(|fact| fact["status"] == "present") {
                pending.extend(class_ancestors(&id, &fact["entity"]).unwrap_or_default());
            }
        }
        let mut pending: Vec<String> = std::mem::take(&mut self.locales).into_iter().collect();
        while let Some(id) = pending.pop() {
            let key = ("entity".to_owned(), id.clone());
            if !facts.contains_key(&key) || !self.locales.insert(id) {
                continue;
            }
            if let Some(fact) = facts.get(&key).filter(|fact| fact["status"] == "present") {
                pending.extend(entity_ids(&fact["entity"], "P131"));
            }
        }
    }
}

/// The view of the places `ids`, and of the facts `named` besides them, in `root`.
fn view_of(
    facts: &Facts,
    root: &Path,
    ids: &[String],
    named: &BTreeSet<(String, String)>,
) -> Result<(Absent, View), String> {
    fs::create_dir_all(root).map_err(|e| e.to_string())?;
    let mut absent = Absent::new();
    let mut sources = Vec::new();
    // The paths of `sources` that a later source can name again.
    let mut listed = BTreeSet::new();
    let mut needed = Needed::default();
    let mut places = Vec::new();
    let mut aliases = BTreeMap::new();
    for id in ids {
        let Some(entity) = subject(facts, id) else {
            if !facts.contains_key(&("entity".into(), id.clone())) {
                lacks(&mut absent, "entity");
            }
            continue;
        };
        let canonical = string(&entity, "id")?.to_owned();
        if canonical != *id {
            aliases.insert(id.clone(), canonical.clone());
        }
        needed.classes.extend(entity_ids(&entity, "P31"));
        needed.locales.extend(entity_ids(&entity, "P131").into_iter().chain(entity_ids(&entity, "P17")));
        needed.entities.extend([id.clone(), canonical.clone()]);
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
                articles.push(serde_json::json!({"language":language,"title":article["title"],"path":needed.path(facts, &key),"revision":article["revision"],"url":article["url"],"compact":true,"requested_title":title,"aliases":article["aliases"]}));
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
            if let Some(filename) = facts.get(&key).as_ref().and_then(lead_image) {
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
            listings.push(serde_json::json!({"title":category,"complete":!listing["truncated"].as_bool().unwrap_or(false),"limit":listing["limit"],"pages":[{"path":needed.path(facts, &key),"continuation":null}]}));
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
                let mut image = serde_json::json!({"filename":filename,"metadata_path":needed.path(facts, &key),"source":source,"language":language});
                if let Some(path) = needed.path(facts, &("mediainfo".into(), format!("M{}", metadata["pageid"]))) {
                    image["depicts_path"] = serde_json::json!(path);
                }
                if let Some(file) = facts.get(&("file".into(), key.1.clone())) {
                    needed.path(facts, &("file".into(), key.1.clone()));
                    needed.assets.insert(string(&file["asset"], "sha256")?.to_owned());
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
                            if listed.insert(path.clone()) {
                                let url = "https://commons.wikimedia.org/w/api.php".into();
                                sources.push(ViewSource {
                                    path: path.clone(),
                                    sha256: hash(&data),
                                    bytes: data.len() as u64,
                                    url,
                                });
                            }
                            image[format!("{witness}_path")] = serde_json::json!(path);
                        }
                    }
                }
                images.push(image);
            }
        }
        places.push(serde_json::json!({"qid":canonical,"entity_path":needed.path(facts, &("entity".into(), id.clone())).or_else(|| needed.path(facts, &("entity".into(), canonical.clone()))),"articles":articles,"images":images,"commons_categories":listings}));
    }
    needed.facts.extend(named.iter().cloned());
    needed.close(facts);
    let mut listing = Vec::new();
    for (name, entry) in &facts.entries {
        let key = (entry.kind.clone(), entry.key.clone());
        let entity = entry.kind == "entity";
        let class = needed.facts.contains(&key);
        let (locale, by_identity) =
            (entity && needed.locales.contains(&entry.key), entity && needed.entities.contains(&entry.key));
        if !class && !locale && !by_identity {
            continue;
        }
        let path = &facts.files[name];
        // The name of a fact file is the SHA-256 of its bytes, which `facts` checked.
        let sha256 = Path::new(name).file_stem().and_then(|stem| stem.to_str()).unwrap_or_default();
        let bytes = fs::metadata(path).map_err(|e| e.to_string())?.len();
        let source =
            |relative: String, url: &str| ViewSource { path: relative, sha256: sha256.into(), bytes, url: url.into() };
        if locale {
            let relative = format!("locales/{}.json", entry.key);
            place(path, &root.join(&relative))?;
            listing.push(source(relative, "wikimedia:locale"));
        }
        if by_identity {
            let relative = format!("entities/{}.json", entry.key);
            place(path, &root.join(&relative))?;
            listed.insert(relative.clone());
            listing.push(source(relative, "wikimedia:entity"));
            let canonical = entry.identity.as_deref().unwrap_or(&entry.key);
            if canonical != entry.key && !facts.contains_key(&("entity".into(), canonical.into())) {
                let relative = format!("entities/{canonical}.json");
                if listed.insert(relative.clone()) {
                    place(path, &root.join(&relative))?;
                    listing.push(source(relative, "wikimedia:entity"));
                }
            }
        }
        if class {
            let relative = if entity { format!("classes/{}.json", entry.key) } else { name.clone() };
            place(path, &root.join(&relative))?;
            listing.push(source(relative, entry.url.as_deref().unwrap_or("wikimedia:content")));
        }
    }
    for (name, path) in facts.files.iter().filter(|(name, _)| name.starts_with("content/assets/")) {
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let digest = hash(&bytes);
        if !needed.assets.contains(&digest) {
            continue;
        }
        place(path, &root.join(name))?;
        let file = facts.assets.get(&digest).and_then(|key| facts.get(&("file".into(), key.clone())));
        let file = file.ok_or("unregistered shared media asset")?;
        let url = string(&file["asset"], "url")?.to_owned();
        listing.push(ViewSource { path: name.clone(), sha256: digest, bytes: bytes.len() as u64, url });
    }
    listing.extend(sources);
    let coverage = serde_json::json!({"entity_coverage_complete":true,"asset_phase_complete":true});
    let coverage = coverage.as_object().cloned().unwrap_or_default();
    Ok((absent, View { sources: listing, places, aliases, omissions: Vec::new(), coverage, peaks: None }))
}

pub fn landmark_view(
    files: &BTreeMap<String, std::path::PathBuf>,
    root: &Path,
    ids: &[String],
) -> Result<Absent, String> {
    let facts = facts(files)?;
    let chosen = selected(&facts, ids);
    let (mut absent, mut view) = view_of(&facts, root, &chosen.iter().cloned().collect::<Vec<_>>(), &BTreeSet::new())?;
    for id in ids.iter().filter(|id| !chosen.contains(*id)) {
        let Some(name) = facts.names.get(&("entity".into(), id.clone())) else {
            lacks(&mut absent, "entity");
            continue;
        };
        let reason =
            if facts.entries[name].present { "policy_exclusion_or_no_category" } else { "confirmed_missing_entity" };
        view.omissions.push(serde_json::json!({"qid":id,"asset":"site","reason":reason}));
    }
    view.coverage.insert("candidate_identities".into(), serde_json::json!(ids.len()));
    view.write(root)?;
    Ok(absent)
}

pub fn peak_view(files: &BTreeMap<String, std::path::PathBuf>, root: &Path) -> Result<Absent, String> {
    let source: peaks::SummitSource =
        serde_json::from_slice(&fs::read(root.join("summits.json")).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let facts = facts(files)?;
    let (mut ids, mut absent, mut links) = (BTreeSet::new(), Absent::new(), BTreeSet::new());
    let mut resolutions = Vec::new();
    for summit in &source.summits {
        for kind in ["wikidata", "wikipedia"] {
            let Some(tag) = summit.tags.get(kind) else { continue };
            let key = ("link".to_owned(), format!("{kind}:{tag}"));
            let Some(fact) = facts.get(&key) else {
                lacks(&mut absent, "link");
                continue;
            };
            // The kind is in the fact, not in its file name.
            let path = facts.names.get(&key).cloned().ok_or("missing link file")?;
            if fact["status"] == "present" {
                ids.insert(string(&fact, "identity")?.to_owned());
            }
            resolutions.push(serde_json::json!({"node_id":summit.node_id,"kind":kind,"path":path,"status":if fact["status"] == "present" {"resolved"} else {"confirmed_missing"}}));
            links.insert(key);
        }
    }
    let (lacking, mut view) = view_of(&facts, root, &ids.into_iter().collect::<Vec<_>>(), &links)?;
    for (kind, count) in lacking {
        *absent.entry(kind).or_default() += count;
    }
    let data = fs::read(root.join("summits.json")).map_err(|e| e.to_string())?;
    let url = "osm:regional-summits".into();
    view.sources.push(ViewSource { path: "summits.json".into(), sha256: hash(&data), bytes: data.len() as u64, url });
    view.peaks = Some(serde_json::json!({"summits_path":"summits.json","resolutions":resolutions}));
    view.write(root)?;
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
            let mut unrelated = facts(&files).unwrap().get(&("article".into(), "en:Old Castle".into())).unwrap();
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
        let sources: Sources = serde_json::from_value(manifest["sources"].clone()).unwrap();
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
