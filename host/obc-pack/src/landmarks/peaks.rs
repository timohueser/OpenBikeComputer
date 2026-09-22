//! OSM summit discovery and a separate article catalogue for Peak View.
use super::*;
use osmpbf::{Element, ElementReader};

#[derive(Debug, Serialize, Deserialize)]
pub struct Summit {
    pub node_id: i64,
    pub latitude: f64,
    pub longitude: f64,
    pub tags: BTreeMap<String, String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SummitSource {
    pub schema: u32,
    pub osm_sha256: String,
    pub summits: Vec<Summit>,
}

#[derive(Debug, Deserialize)]
pub(super) struct PeakCapture {
    summits_path: String,
    resolutions: Vec<Resolution>,
}

#[derive(Debug, Deserialize)]
struct Resolution {
    node_id: i64,
    kind: String,
    path: Option<String>,
    status: String,
    canonical_path: Option<String>,
    canonical_language: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Association {
    pub node_id: i64,
    pub article_id: String,
    pub latitude: f64,
    pub longitude: f64,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeakArticle {
    pub id: String,
    #[serde(flatten)]
    pub article: PreparedArticle,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PeakContent {
    pub schema: u32,
    pub collection: String,
    pub input_sha256: String,
    pub policy_sha256: String,
    pub languages: Vec<String>,
    pub source_coverage: Value,
    pub counts: Counts,
    pub associations: Vec<Association>,
    pub records: Vec<PeakArticle>,
    pub omissions: Vec<Omission>,
    /// Absent from a compiled catalogue: a production compile has every original it ranked.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub photo_requests: Vec<PhotoRequest>,
}

pub(super) fn is_summit(tags: &BTreeMap<String, String>) -> bool {
    crate::poi::classify(tags.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .is_some_and(|p| p.subtype == obc_formats::obcm::SUMMIT_SUBTYPE_ID)
}

/// Read only the named summit nodes accepted by the map's POI classifier.
/// The derived source keeps original tags and the digest of its upstream PBF.
pub fn discover(osm: &Path, boundary: &Path, output: &Path) -> Result<(), String> {
    let polygon = boundary_geometry(
        &serde_json::from_slice(&fs::read(boundary).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?,
    )?;
    let mut summits = BTreeMap::new();
    let mut error = None;
    let mut node = |id, lat, lon, tags: BTreeMap<String, String>| {
        if !is_summit(&tags) {
            return;
        }
        let latitude = lat as f64 / 1e7;
        let longitude = lon as f64 / 1e7;
        let covered =
            Geometry::new_from_wkt(&format!("POINT ({longitude} {latitude})")).and_then(|point| polygon.covers(&point));
        match covered {
            Ok(true) => {
                if summits.insert(id, Summit { node_id: id, latitude, longitude, tags }).is_some() {
                    error = Some("duplicate OSM summit node".to_owned());
                }
            }
            Ok(false) => (),
            Err(e) => error = Some(e.to_string()),
        }
    };
    ElementReader::from_path(osm)
        .map_err(|e| e.to_string())?
        .for_each(|element| match element {
            Element::Node(n) => node(
                n.id(),
                n.decimicro_lat(),
                n.decimicro_lon(),
                n.tags().map(|(k, v)| (k.into(), v.into())).collect(),
            ),
            Element::DenseNode(n) => node(
                n.id(),
                n.decimicro_lat(),
                n.decimicro_lon(),
                n.tags().map(|(k, v)| (k.into(), v.into())).collect(),
            ),
            _ => (),
        })
        .map_err(|e| e.to_string())?;
    if let Some(error) = error {
        return Err(error);
    }
    let source = SummitSource { schema: 1, osm_sha256: file_digest(osm)?, summits: summits.into_values().collect() };
    fs::write(output, serde_json::to_vec_pretty(&source).map_err(|e| e.to_string())?).map_err(|e| e.to_string())
}

fn wikipedia(value: &str) -> Option<(&str, &str)> {
    let (language, title) = value.split_once(':')?;
    (language.len() >= 2
        && language.len() <= 12
        && language.bytes().all(|b| b.is_ascii_lowercase() || b == b'-')
        && !title.trim().is_empty()
        && !title.starts_with("//"))
    .then_some((language, title))
}

/// Resolve only the captured response for the OSM tag. Display names and coordinates
/// never participate in article identity.
fn identity(
    root: &Path,
    sources: &[Source],
    summit: &Summit,
    resolution: &Resolution,
) -> Result<(String, Option<Value>), String> {
    let raw = json_pinned(root, sources, resolution.path.as_deref().ok_or("link_resolution_missing")?)?;
    match resolution.kind.as_str() {
        "wikidata" => {
            let original = summit.tags.get("wikidata").filter(|id| is_qid(id)).ok_or("invalid_wikidata_link")?;
            let value = &raw["entities"][original];
            let canonical = string(value, "id")?;
            if !is_qid(canonical)
                || value.get("missing").is_some()
                || (canonical != original
                    && (value["redirects"]["from"] != *original || value["redirects"]["to"] != canonical))
            {
                return Err("wikidata_resolution_mismatch".into());
            }
            Ok((canonical.into(), None))
        }
        "wikipedia" => {
            let (language, title) =
                summit.tags.get("wikipedia").and_then(|v| wikipedia(v)).ok_or("invalid_wikipedia_link")?;
            let page = assets::resolved_page(&raw, title)?;
            if let Some(id) = page["pageprops"]["wikibase_item"].as_str().filter(|id| is_qid(id)) {
                Ok((id.into(), None))
            } else {
                if raw.get("continue").is_some() {
                    return Err("incomplete_language_links".into());
                }
                let mut links = BTreeMap::new();
                for link in page["langlinks"].as_array().into_iter().flatten() {
                    links.insert(string(link, "lang")?.to_owned(), string(link, "*")?.to_owned());
                }
                links.insert(language.to_owned(), string(page, "title")?.to_owned());
                let supported: Vec<_> = locale::languages()
                    .into_iter()
                    .filter_map(|(code, _)| links.get(&code).map(|title| (code, title)))
                    .collect();
                let (selected, title) = supported.first().ok_or("no_supported_language_link")?;
                if resolution.canonical_language.as_ref() != Some(selected) {
                    return Err("canonical_language_mismatch".into());
                }
                let canonical_raw = if selected == language {
                    raw.clone()
                } else {
                    json_pinned(root, sources, resolution.canonical_path.as_deref().ok_or("canonical_page_missing")?)?
                };
                let canonical_page = assets::resolved_page(&canonical_raw, title)?;
                if let Some(id) = canonical_page["pageprops"]["wikibase_item"].as_str().filter(|id| is_qid(id)) {
                    return Ok((id.into(), None));
                }
                let page_id = canonical_page["pageid"].as_u64().filter(|id| *id > 0).ok_or("wikipedia_page_missing")?;
                let id = format!("wiki-{selected}-{page_id}");
                if canonical_raw.get("continue").is_some() {
                    return Err("incomplete_language_links".into());
                }
                let mut links = BTreeMap::new();
                for link in canonical_page["langlinks"].as_array().into_iter().flatten() {
                    links.insert(string(link, "lang")?.to_owned(), string(link, "*")?.to_owned());
                }
                links.insert(selected.clone(), string(canonical_page, "title")?.to_owned());
                let mut value = serde_json::json!({"id": id, "labels": {}, "sitelinks": {}});
                for (code, title) in
                    locale::languages().into_iter().filter_map(|(code, _)| links.get(&code).map(|title| (code, title)))
                {
                    value["labels"][&code] = serde_json::json!({"value": title});
                    value["sitelinks"][format!("{code}wiki")] = serde_json::json!({"title": title});
                }
                Ok((id, Some(value)))
            }
        }
        _ => Err("invalid_link_kind".into()),
    }
}

/// Compile peak records once per canonical identity, preserving every OSM association.
/// This format cannot be passed to the landmark map serializer.
pub fn compile(
    snapshot_path: &Path,
    boundary: &Path,
    output: &Path,
    select_photos: bool,
) -> Result<PeakContent, String> {
    let (snapshot, mut input) = load_snapshot(snapshot_path)?;
    let root = snapshot_path.parent().ok_or("snapshot has no parent")?;
    let capture = snapshot.peaks.as_ref().ok_or("snapshot has no peak collection")?;
    let summit_bytes = read_pinned(root, &snapshot.sources, &capture.summits_path, 16 * 1024 * 1024)?;
    let source: SummitSource = serde_json::from_slice(&summit_bytes).map_err(|e| e.to_string())?;
    if source.schema != 1 || source.osm_sha256.len() != 64 {
        return Err("invalid summit source".into());
    }
    let boundary_bytes = fs::read(boundary).map_err(|e| e.to_string())?;
    let polygon = boundary_geometry(&serde_json::from_slice(&boundary_bytes).map_err(|e| e.to_string())?)?;
    input.extend(boundary_bytes);
    let locales = load_locales(root, &snapshot.sources)?;
    let mut policy = include_bytes!("peaks.rs").to_vec();
    for bytes in [
        include_bytes!("mod.rs").as_slice(),
        include_bytes!("assets.rs"),
        include_bytes!("photo.rs"),
        include_bytes!("text.rs"),
        include_bytes!("locale.rs"),
        include_bytes!("../poi.rs"),
        include_bytes!("../../../../Cargo.lock"),
        locale::LANGUAGE_BYTES,
        PHOTO_POOL_BYTES,
    ] {
        policy.extend(bytes);
    }
    let mut result = PeakContent {
        schema: 1,
        collection: "peaks".into(),
        input_sha256: hash(&input),
        policy_sha256: hash(&policy),
        languages: locale::languages().into_iter().map(|(code, _)| code).collect(),
        source_coverage: snapshot.coverage,
        counts: Counts { captured: source.summits.len(), ..Counts::default() },
        associations: Vec::new(),
        records: Vec::new(),
        omissions: Vec::new(),
        photo_requests: Vec::new(),
    };
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    if fs::read_dir(output).map_err(|e| e.to_string())?.next().is_some() {
        return Err("peak output directory must be empty".into());
    }
    let mut associations = BTreeMap::new();
    let mut entities = BTreeMap::new();
    let mut seen = BTreeSet::new();
    for summit in source.summits {
        if !seen.insert(summit.node_id) {
            return Err("duplicate source summit".into());
        }
        if summit.node_id <= 0
            || !is_summit(&summit.tags)
            || !(-90.0..=90.0).contains(&summit.latitude)
            || !(-180.0..=180.0).contains(&summit.longitude)
        {
            return Err("invalid source summit".into());
        }
        let point = Geometry::new_from_wkt(&format!("POINT ({} {})", summit.longitude, summit.latitude))
            .map_err(|e| e.to_string())?;
        if !polygon.covers(&point).map_err(|e| e.to_string())? {
            continue;
        }
        let mut omit = |reason: String| {
            result.omissions.push(Omission {
                qid: format!("osm-node-{}", summit.node_id),
                asset: "link".into(),
                reason,
            })
        };
        if !summit.tags.contains_key("wikidata") && !summit.tags.contains_key("wikipedia") {
            omit("no_explicit_link".into());
            continue;
        }
        result.counts.candidates += 1;
        let resolutions: Vec<_> = capture.resolutions.iter().filter(|r| r.node_id == summit.node_id).collect();
        let mut identities = BTreeMap::new();
        for resolution in resolutions {
            if resolution.status != "resolved" {
                omit(resolution.status.clone());
                continue;
            }
            match identity(root, &snapshot.sources, &summit, resolution) {
                Ok((id, entity)) => {
                    identities.insert(id, entity);
                }
                Err(reason) => omit(reason),
            }
        }
        if identities.len() != 1 {
            omit(if identities.is_empty() { "no_resolved_link" } else { "conflicting_explicit_links" }.into());
            continue;
        }
        let (id, direct_entity) = identities.pop_first().unwrap();
        let entity = if let Some(entity) = direct_entity {
            entity
        } else {
            let path = format!("entities/{id}.json");
            if !snapshot.sources.iter().any(|s| s.path == path) {
                omit("entity_acquisition_failed".into());
                continue;
            }
            let raw = json_pinned(root, &snapshot.sources, &path)?;
            let entity = raw["entities"][&id].clone();
            if entity["id"] != id {
                omit("entity_identity_mismatch".into());
                continue;
            }
            entity
        };
        if entities.get(&id).is_some_and(|previous| previous != &entity) {
            return Err(format!("conflicting article identity: {id}"));
        }
        entities.insert(id.clone(), entity);
        associations.entry(id.clone()).or_insert_with(Vec::new).push(Association {
            node_id: summit.node_id,
            article_id: id,
            latitude: summit.latitude,
            longitude: summit.longitude,
        });
    }
    for (id, entity) in entities {
        let place =
            snapshot.places.iter().find(|p| p["qid"] == id).ok_or_else(|| format!("missing peak capture {id}"))?;
        // The summit's own OSM coordinate, not the article entity's: a photo taken at the summit
        // shows the view from the peak, which does not help a rider identify it.
        let summit = associations.get(&id).and_then(|linked| linked.first()).map(|a| (a.latitude, a.longitude));
        if let Some(article) = prepare_article(
            &Inputs { root, sources: &snapshot.sources, locales: &locales, output, select_photos },
            place,
            &entity,
            &mut Found { omissions: &mut result.omissions, requests: &mut result.photo_requests },
            summit,
        )? {
            result.counts.texts += 1;
            if let Some(photo) = &article.photo {
                result.counts.images += 1;
                result.counts.photo_bytes += photo.bytes;
            }
            result.records.push(PeakArticle { id: id.clone(), article });
            result.associations.extend(associations.remove(&id).unwrap());
        }
    }
    result.associations.sort_by_key(|a| a.node_id);
    result.omissions.sort_by(|a, b| (&a.qid, &a.asset, &a.reason).cmp(&(&b.qid, &b.asset, &b.reason)));
    fs::write(output.join("peaks.json"), serde_json::to_vec_pretty(&result).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(result)
}
