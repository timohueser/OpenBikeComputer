//! Offline landmark preparation from digest-pinned source responses. Device encoding is owned
//! by the map serializer; this module emits bounded text, attribution and RGB222 assets.

mod assets;
pub mod discover;
mod locale;
pub mod peaks;
mod photo;
mod policy;
pub mod text;

/// The shared UI language set, verbatim. It decides which articles are fetched and which places
/// are eligible at all, so a stage that caches captures has to key on it.
pub use locale::LANGUAGE_BYTES;
/// The curated category policy, verbatim. Capture discovers from the same bytes the compiler
/// selects with, so the two cannot drift apart.
pub use policy::BYTES as POLICY_BYTES;

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
    #[serde(default)]
    peaks: Option<peaks::PeakCapture>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Content {
    pub schema: u32,
    pub input_sha256: String,
    pub policy_sha256: String,
    pub category_policy_sha256: String,
    pub languages: Vec<String>,
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
    pub default_language: String,
    pub fallback_sources: Vec<String>,
    pub variants: Vec<TextVariant>,
    pub photo: Option<Photo>,
}
#[derive(Debug, Serialize, Deserialize)]
pub struct TextVariant {
    pub language: String,
    pub text_pages: Vec<String>,
    pub attribution: Attribution,
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
/// `Q` and a decimal with no leading zero and no sign — the rule the capture tool applies to a
/// candidate list. The two must agree: a QID one side accepts and the other refuses is a region
/// that cannot be captured.
fn is_qid(id: &str) -> bool {
    match id.strip_prefix('Q').map(str::as_bytes) {
        Some([b'1'..=b'9', rest @ ..]) => rest.iter().all(u8::is_ascii_digit),
        _ => false,
    }
}

/// Numeric by QID, which is the order the capture asks for entities in.
fn qid_order(qid: &str) -> (usize, &str) {
    let digits = qid.strip_prefix('Q').unwrap_or(qid);
    (digits.len(), digits)
}
fn file_digest(path: &Path) -> Result<String, String> {
    use std::io::Read as _;
    let mut file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|e| format!("{}: {e}", path.display()))?;
        if count == 0 {
            return Ok(digest.finalize().iter().map(|byte| format!("{byte:02x}")).collect());
        }
        digest.update(&buffer[..count]);
    }
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
/// The largest pinned JSON response the compiler reads. The capture tool asks for a
/// `wbgetentities` batch again in halves when its response passes this, so every response a
/// capture keeps is one the compiler can read.
const MAX_JSON_SOURCE: u64 = 16 * 1024 * 1024;

fn json_pinned(root: &Path, sources: &[Source], path: &str) -> Result<Value, String> {
    serde_json::from_slice(&read_pinned(root, sources, path, MAX_JSON_SOURCE)?)
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
fn class_ancestors(id: &str, entity: &Value) -> Result<Vec<String>, String> {
    if let Some(redirect) = entity.get("redirects") {
        let target = string(redirect, "to")?;
        if redirect["from"] != id || entity["id"] != target || !is_qid(target) {
            return Err(format!("invalid captured class redirect: {id}"));
        }
        return Ok(vec![target.to_owned()]);
    }
    Ok(entity_ids(entity, "P279"))
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
pub fn compile(snapshot_path: &Path, boundary: &Path, output: &Path) -> Result<Content, String> {
    let (snapshot, raw) = load_snapshot(snapshot_path)?;
    if snapshot.peaks.is_some() {
        return Err("peak capture requires the peak compiler".into());
    }
    let root = snapshot_path.parent().ok_or("snapshot has no parent")?;
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
            let mut ancestors = class_ancestors(id, entity)?;
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
    policy_input.extend_from_slice(locale::LANGUAGE_BYTES);
    policy_input.extend_from_slice(include_bytes!("locale.rs"));
    let locales = load_locales(root, &snapshot.sources)?;
    let mut input = raw;
    input.extend_from_slice(&boundary_bytes);
    let mut content = Content {
        schema: 2,
        input_sha256: hash(&input),
        policy_sha256: hash(&policy_input),
        category_policy_sha256: hash(policy::BYTES),
        languages: locale::languages().into_iter().map(|(code, _)| code).collect(),
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
    // The capture batches entities in numeric QID order and a response holds fifty of them, so
    // the compiler reads places in that same order. One loaded response then serves fifty places
    // and is never returned to: the batch index of the places it visits never decreases.
    let mut places = snapshot.places;
    places.sort_by_key(|place| {
        let (length, digits) = qid_order(place["qid"].as_str().unwrap_or(""));
        (length, digits.to_owned())
    });
    let mut seen = BTreeSet::new();
    let mut loaded: Option<(String, Value)> = None;
    for place in places {
        let qid = string(&place, "qid")?.to_owned();
        if !is_qid(&qid) {
            return Err("invalid QID".into());
        }
        if !seen.insert(qid.clone()) {
            continue;
        }
        let mut omit = |asset: &str, reason: String| {
            content.omissions.push(Omission { qid: qid.clone(), asset: asset.into(), reason })
        };
        // A place names the response its entity arrived in; a capture that asked for the entity
        // alone names none, and it is at the one-entity path.
        let path = match place.get("entity_path").and_then(Value::as_str) {
            Some(path) => path.to_owned(),
            None => format!("entities/{qid}.json"),
        };
        if !matches!(&loaded, Some((held, _)) if *held == path) {
            let raw = json_pinned(root, &snapshot.sources, &path)?;
            loaded = Some((path, raw));
        }
        let entity = &loaded.as_ref().expect("the response just loaded").1["entities"][&qid];
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
        let Some(PreparedArticle { name, default_language, fallback_sources, variants, photo }) =
            prepare_article(root, &snapshot.sources, &place, entity, &locales, output, &mut content.omissions)?
        else {
            continue;
        };
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
            default_language,
            fallback_sources,
            variants,
            photo,
        });
    }
    fs::write(output.join("content.json"), serde_json::to_vec_pretty(&content).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    Ok(content)
}

fn boundary_geometry(value: &Value) -> Result<Geometry, String> {
    if value["type"] == "Feature" {
        return boundary_geometry(&value["geometry"]);
    }
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

#[derive(Debug, Serialize, Deserialize)]
pub struct PreparedArticle {
    pub name: String,
    pub default_language: String,
    pub fallback_sources: Vec<String>,
    pub variants: Vec<TextVariant>,
    pub photo: Option<Photo>,
}

fn prepare_article(
    root: &Path,
    sources: &[Source],
    place: &Value,
    entity: &Value,
    locales: &BTreeMap<String, Value>,
    output: &Path,
    omissions: &mut Vec<Omission>,
) -> Result<Option<PreparedArticle>, String> {
    let qid = string(place, "qid")?;
    let mut omit = |asset: &str, reason: String| {
        omissions.push(Omission { qid: qid.into(), asset: asset.into(), reason });
    };
    let captures = place["articles"].as_array().ok_or("article captures missing")?;
    let mut variants = Vec::new();
    let mut lead = BTreeSet::new();
    for (candidate, _) in locale::languages() {
        if let Some(capture) = captures.iter().find(|item| item["language"] == candidate) {
            match article(root, sources, entity, capture) {
                Ok(Article { language, pages, attribution, lead_image }) => {
                    lead.extend(lead_image);
                    variants.push(TextVariant { language, text_pages: pages, attribution });
                }
                Err(reason) => omit("article", format!("{candidate}: {reason}")),
            }
        }
    }
    if variants.is_empty() {
        omit("article", "no_usable_captured_language".into());
        return Ok(None);
    }
    let (default_language, fallback_sources) = locale::default_language(entity, locales, &variants);
    let name = entity["labels"][&default_language]["value"]
        .as_str()
        .or_else(|| entity["labels"]["en"]["value"].as_str())
        .or_else(|| place["name"].as_str())
        .ok_or("site name missing")?;
    let name = text::normalize(name);
    if !text::supported(&name) {
        omit("site", "name_glyph".into());
        return Ok(None);
    }
    let mut images: Vec<_> = place["images"].as_array().into_iter().flatten().collect();
    images.sort_by_key(|image| (image["source"] != "P18", image["filename"].as_str().unwrap_or("").replace('_', " ")));
    let mut photo = None;
    let p18: BTreeSet<_> = claims(entity, "P18")
        .filter_map(|claim| claim["mainsnak"]["datavalue"]["value"].as_str())
        .map(|file| file.replace('_', " "))
        .collect();
    for image in images {
        let allowed = match image["source"].as_str() {
            Some("P18") => &p18,
            Some("wikipedia-lead") => &lead,
            _ => {
                omit("photo", "photo_source_kind".into());
                continue;
            }
        };
        match assets::photo(root, sources, image, allowed, qid) {
            Ok((candidate, pixels))
                if variants.iter().all(|v| {
                    candidate.attribution.display_pages.len() + v.attribution.display_pages.len()
                        <= text::MAX_SOURCE_PAGES
                }) =>
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
    Ok(Some(PreparedArticle { name, default_language, fallback_sources, variants, photo }))
}

fn load_snapshot(snapshot_path: &Path) -> Result<(Snapshot, Vec<u8>), String> {
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
    Ok((snapshot, raw))
}

fn load_locales(root: &Path, sources: &[Source]) -> Result<BTreeMap<String, Value>, String> {
    let mut locales = BTreeMap::new();
    for source in sources.iter().filter(|s| s.path.starts_with("locales/")) {
        let raw = json_pinned(root, sources, &source.path)?;
        let id = Path::new(&source.path).file_stem().and_then(|s| s.to_str()).ok_or("invalid locale path")?;
        if let Some(entity) = raw["entities"].get(id).filter(|e| e["id"] == id) {
            locales.insert(id.to_owned(), entity.clone());
        }
    }
    Ok(locales)
}

#[cfg(test)]
mod peaks_tests;
