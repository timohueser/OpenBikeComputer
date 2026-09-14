//! Offline landmark preparation from digest-pinned source responses. Device encoding is owned
//! by the map serializer; this module emits bounded text, attribution and RGB222 assets.

mod assets;
mod photo;
mod policy;
pub mod text;

use assets::{article, Article};
use geos::{Geom as _, Geometry};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path},
};

const COMPILER_POLICY: &str = "landmarks-1;lead-2-sentences;latin-extended-a;label-216x240;rgba-white-lanczos3-bayer4;credits-8192;decode-32MiB-16384-128MiB";
const FALLBACK: [&str; 5] = ["en", "de", "fr", "it", "ga"];

#[derive(Deserialize)]
struct Source {
    path: String,
    sha256: String,
    bytes: u64,
    url: String,
}
#[derive(Deserialize)]
struct Snapshot {
    schema: u32,
    sources: Vec<Source>,
    places: Vec<Value>,
    #[serde(default)]
    coverage: Value,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Content {
    pub schema: u32,
    pub input_sha256: String,
    pub policy_sha256: String,
    pub category_policy_sha256: String,
    pub language: String,
    pub source_coverage: Value,
    pub counts: Counts,
    pub candidate_qids: Vec<String>,
    pub records: Vec<Record>,
    pub omissions: Vec<Omission>,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Counts {
    pub captured: usize,
    pub candidates: usize,
    pub texts: usize,
    pub images: usize,
    pub mapped_approaches: Option<usize>,
    pub photo_bytes: usize,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Record {
    pub qid: String,
    pub name: String,
    pub category: u8,
    pub latitude: f64,
    pub longitude: f64,
    pub language: String,
    pub text_pages: Vec<String>,
    pub article: Attribution,
    pub photo: Option<Photo>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Attribution {
    pub source_url: String,
    pub revision: String,
    pub license_url: String,
    pub original_notices: String,
    pub display_pages: Vec<String>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Photo {
    pub path: String,
    pub sha256: String,
    pub bytes: usize,
    pub attribution: Attribution,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct Omission {
    pub qid: String,
    pub asset: String,
    pub reason: String,
}

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|byte| format!("{byte:02x}")).collect()
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value.get(key).and_then(Value::as_str).ok_or_else(|| format!("missing {key}"))
}
fn read_pinned(root: &Path, sources: &[Source], path: &str, limit: u64) -> Result<Vec<u8>, String> {
    if Path::new(path).components().any(|part| !matches!(part, Component::Normal(_))) {
        return Err(format!("source path must be relative: {path}"));
    }
    let source =
        sources.iter().find(|source| source.path == path).ok_or_else(|| format!("unregistered source: {path}"))?;
    if source.bytes > limit {
        return Err(format!("source exceeds byte limit: {path}"));
    }
    let file = root.join(path);
    if !file
        .canonicalize()
        .map_err(|e| format!("missing source {path}: {e}"))?
        .starts_with(root.canonicalize().map_err(|e| e.to_string())?)
    {
        return Err(format!("source path escapes snapshot: {path}"));
    }
    let metadata = fs::symlink_metadata(&file).map_err(|e| format!("missing source {path}: {e}"))?;
    if !metadata.is_file() || metadata.len() != source.bytes {
        return Err(format!("source size changed: {path}"));
    }
    let bytes = fs::read(&file).map_err(|e| format!("read {path}: {e}"))?;
    if hash(&bytes) != source.sha256 {
        return Err(format!("source checksum changed: {path}"));
    }
    Ok(bytes)
}
fn json_pinned(root: &Path, sources: &[Source], path: &str) -> Result<Value, String> {
    serde_json::from_slice(&read_pinned(root, sources, path, 16 * 1024 * 1024)?)
        .map_err(|e| format!("invalid source {path}: {e}"))
}
fn claims<'a>(entity: &'a Value, property: &str) -> impl Iterator<Item = &'a Value> {
    let claims = entity["claims"][property].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let preferred = claims.iter().any(|claim| claim["rank"] == "preferred");
    claims.iter().filter(move |claim| claim["rank"] != "deprecated" && (!preferred || claim["rank"] == "preferred"))
}
fn entity_ids(entity: &Value, property: &str) -> Vec<String> {
    claims(entity, property)
        .filter_map(|claim| claim["mainsnak"]["datavalue"]["value"]["id"].as_str().map(str::to_owned))
        .collect()
}
fn coordinate(entity: &Value) -> Option<(f64, f64)> {
    let values: Vec<_> = claims(entity, "P625")
        .filter_map(|claim| {
            let value = &claim["mainsnak"]["datavalue"]["value"];
            if value["globe"].as_str()? != "http://www.wikidata.org/entity/Q2" {
                return None;
            }
            let (lat, lon) = (value["latitude"].as_f64()?, value["longitude"].as_f64()?);
            (lat.is_finite() && lon.is_finite() && (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon))
                .then_some((lat, lon))
        })
        .collect();
    let first = *values.first()?;
    values.iter().all(|value| *value == first).then_some(first)
}

/// `boundary` is a GeoJSON Polygon/MultiPolygon, not a country-claim filter. All inputs are local.
pub fn compile(snapshot_path: &Path, boundary: &Path, language: &str, output: &Path) -> Result<Content, String> {
    let raw = fs::read(snapshot_path).map_err(|e| e.to_string())?;
    let snapshot: Snapshot = serde_json::from_slice(&raw).map_err(|e| e.to_string())?;
    if snapshot.schema != 1 {
        return Err("unsupported source snapshot schema".into());
    }
    let root = snapshot_path.parent().ok_or("snapshot has no parent")?;
    let mut registered = BTreeSet::new();
    for source in &snapshot.sources {
        if !registered.insert(&source.path) {
            return Err(format!("duplicate source: {}", source.path));
        }
        read_pinned(root, &snapshot.sources, &source.path, photo::MAX_SOURCE_BYTES as u64)?;
    }
    let boundary_bytes = fs::read(boundary).map_err(|e| e.to_string())?;
    let boundary_json: Value = serde_json::from_slice(&boundary_bytes).map_err(|e| e.to_string())?;
    let geometry = if boundary_json["type"] == "Feature" { &boundary_json["geometry"] } else { &boundary_json };
    let polygon = boundary_geometry(geometry)?;
    let mut parents: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for source in
        snapshot.sources.iter().filter(|source| source.path.starts_with("classes/") && source.path.ends_with(".json"))
    {
        let capture = json_pinned(root, &snapshot.sources, &source.path)?;
        let entities = capture["entities"].as_object().ok_or("invalid class capture")?;
        for (id, entity) in entities {
            if entity.get("missing").is_some() {
                continue;
            }
            let mut ancestors = entity_ids(entity, "P279");
            ancestors.sort();
            ancestors.dedup();
            if parents.get(id).is_some_and(|previous| previous != &ancestors) {
                return Err(format!("conflicting captured class revisions: {id}"));
            }
            parents.insert(id.clone(), ancestors);
        }
    }
    let policy = policy::Policy::load();
    let mut policy_input = policy::BYTES.to_vec();
    policy_input.extend_from_slice(COMPILER_POLICY.as_bytes());
    for source in [
        include_bytes!("mod.rs").as_slice(),
        include_bytes!("text.rs"),
        include_bytes!("photo.rs"),
        include_bytes!("assets.rs"),
        include_bytes!("policy.rs"),
        include_bytes!("../../../../Cargo.lock"),
    ] {
        policy_input.extend_from_slice(source);
    }
    policy_input.extend_from_slice(language.as_bytes());
    let mut input = raw;
    input.extend_from_slice(&boundary_bytes);
    let mut content = Content {
        schema: 1,
        input_sha256: hash(&input),
        policy_sha256: hash(&policy_input),
        category_policy_sha256: hash(policy::BYTES),
        language: language.into(),
        source_coverage: snapshot.coverage,
        counts: Counts { captured: snapshot.places.len(), ..Counts::default() },
        candidate_qids: Vec::new(),
        records: Vec::new(),
        omissions: Vec::new(),
    };
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    if fs::read_dir(output).map_err(|e| e.to_string())?.next().is_some() {
        return Err("landmark output directory must be empty".into());
    }
    let mut places = snapshot.places;
    places.sort_by_key(|place| place["qid"].as_str().unwrap_or("").to_owned());
    let mut seen = BTreeSet::new();
    for place in places {
        let qid = string(&place, "qid")?.to_owned();
        if !qid.starts_with('Q') || qid[1..].parse::<u64>().ok().filter(|id| *id != 0).is_none() {
            return Err("invalid QID".into());
        }
        if !seen.insert(qid.clone()) {
            continue;
        }
        let mut omit = |asset: &str, reason: String| {
            content.omissions.push(Omission { qid: qid.clone(), asset: asset.into(), reason })
        };
        let raw_entity = json_pinned(root, &snapshot.sources, &format!("entities/{qid}.json"))?;
        let entity = &raw_entity["entities"][&qid];
        if entity["id"] != qid {
            omit("site", "entity_identity_mismatch".into());
            continue;
        }
        let Some((lat, lon)) = coordinate(entity) else {
            omit("site", "coordinate_missing_or_ambiguous".into());
            continue;
        };
        let point = Geometry::new_from_wkt(&format!("POINT ({lon} {lat})")).map_err(|e| e.to_string())?;
        if !polygon.covers(&point).map_err(|e| e.to_string())? {
            continue;
        }
        let category = match policy.category(&entity_ids(entity, "P31"), &parents) {
            Ok(Some(category)) => category,
            Ok(None) => {
                omit("site", "excluded_type".into());
                continue;
            }
            Err(reason) => {
                omit("site", reason.into());
                continue;
            }
        };
        content.counts.candidates += 1;
        content.candidate_qids.push(qid.clone());
        let mut languages = vec![language];
        for fallback in FALLBACK {
            if !languages.contains(&fallback) {
                languages.push(fallback);
            }
        }
        let captures = place["articles"].as_array().ok_or("article captures missing")?;
        let mut selected = None;
        for candidate in languages {
            if let Some(capture) = captures.iter().find(|item| item["language"] == candidate) {
                match article(root, &snapshot.sources, entity, capture) {
                    Ok(value) => {
                        selected = Some(value);
                        break;
                    }
                    Err(reason) => omit("article", format!("{candidate}: {reason}")),
                }
            }
        }
        let Some(Article { language: actual_language, pages, attribution, lead_image }) = selected else {
            omit("article", "no_usable_captured_language".into());
            continue;
        };
        let name = entity["labels"][&actual_language]["value"]
            .as_str()
            .or_else(|| entity["labels"]["en"]["value"].as_str())
            .ok_or("site name missing")?;
        let name = text::normalize(name);
        if !text::supported(&name) {
            omit("site", "name_glyph".into());
            continue;
        }
        let mut images: Vec<_> = place["images"].as_array().into_iter().flatten().collect();
        images.sort_by_key(|image| {
            (image["source"] != "P18", image["filename"].as_str().unwrap_or("").replace('_', " "))
        });
        let mut photo = None;
        let p18: BTreeSet<_> = claims(entity, "P18")
            .filter_map(|claim| claim["mainsnak"]["datavalue"]["value"].as_str())
            .map(|file| file.replace('_', " "))
            .collect();
        let lead: BTreeSet<_> = lead_image.into_iter().collect();
        for image in images {
            let allowed = match image["source"].as_str() {
                Some("P18") => &p18,
                Some("wikipedia-lead") => &lead,
                _ => {
                    omit("photo", "photo_source_kind".into());
                    continue;
                }
            };
            match assets::photo(root, &snapshot.sources, image, allowed, &qid) {
                Ok((candidate, pixels))
                    if candidate.attribution.display_pages.len() + attribution.display_pages.len()
                        <= text::MAX_SOURCE_PAGES =>
                {
                    fs::write(output.join(&candidate.path), pixels).map_err(|e| e.to_string())?;
                    photo = Some(candidate);
                    break;
                }
                Ok(_) => omit("photo", "attribution_pages".into()),
                Err(reason) => omit("photo", reason),
            }
        }
        if photo.is_none() {
            omit("photo", "no_usable_captured_image".into());
        }
        content.counts.texts += 1;
        if let Some(image) = &photo {
            content.counts.images += 1;
            content.counts.photo_bytes += image.bytes;
        }
        content.records.push(Record {
            qid,
            name,
            category,
            latitude: lat,
            longitude: lon,
            language: actual_language,
            text_pages: pages,
            article: attribution,
            photo,
        });
    }
    fs::write(output.join("content.json"), serde_json::to_vec_pretty(&content).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(content)
}

fn boundary_geometry(value: &Value) -> Result<Geometry, String> {
    if value["type"] == "FeatureCollection" {
        let parts = value["features"]
            .as_array()
            .ok_or("invalid boundary collection")?
            .iter()
            .map(|feature| boundary_geometry(&feature["geometry"]))
            .collect::<Result<Vec<_>, _>>()?;
        if parts.is_empty() {
            return Err("empty boundary collection".into());
        }
        return Geometry::create_geometry_collection(parts).map_err(|e| e.to_string());
    }
    fn ring(value: &Value) -> Result<String, String> {
        let points = value.as_array().ok_or("invalid boundary ring")?;
        if points.len() < 4 || points.first() != points.last() {
            return Err("unclosed boundary ring".into());
        }
        points
            .iter()
            .map(|point| {
                let lon = point[0].as_f64().ok_or("boundary longitude")?;
                let lat = point[1].as_f64().ok_or("boundary latitude")?;
                if !(-180.0..=180.0).contains(&lon) || !(-90.0..=90.0).contains(&lat) {
                    return Err("boundary coordinate range".into());
                }
                Ok(format!("{lon} {lat}"))
            })
            .collect::<Result<Vec<_>, String>>()
            .map(|points| format!("({})", points.join(",")))
    }
    fn polygon(value: &Value) -> Result<String, String> {
        value
            .as_array()
            .ok_or("invalid boundary polygon")?
            .iter()
            .map(ring)
            .collect::<Result<Vec<_>, _>>()
            .map(|rings| format!("({})", rings.join(",")))
    }
    let wkt = match value["type"].as_str() {
        Some("Polygon") => format!("POLYGON {}", polygon(&value["coordinates"])?),
        Some("MultiPolygon") => format!(
            "MULTIPOLYGON ({})",
            value["coordinates"]
                .as_array()
                .ok_or("invalid multipolygon")?
                .iter()
                .map(polygon)
                .collect::<Result<Vec<_>, _>>()?
                .join(",")
        ),
        _ => return Err("boundary must be Polygon or MultiPolygon".into()),
    };
    let geometry = Geometry::new_from_wkt(&wkt).map_err(|e| e.to_string())?;
    if !geometry.is_valid().map_err(|e| e.to_string())? {
        return Err("invalid boundary topology".into());
    }
    Ok(geometry)
}

#[cfg(test)]
mod tests;
