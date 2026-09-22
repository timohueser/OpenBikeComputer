//! Join compiled content to explicit OSM approaches and encode one map section.

use crate::{
    hours::Schedule,
    landmarks::{Attribution, Content, Photo},
    poi::LandmarkLink,
};
use obc_formats::obcm::{landmarks::*, POI_HOURS_REF_NONE};
use sha2::{Digest, Sha256};
use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone)]
pub struct Landmark {
    pub record: LandmarkRecord,
    pub hours: Option<Schedule>,
    pub content: [Vec<u8>; 4],
}

/// Declared photo digests are part of content.json; load verifies their bytes.
/// Encoder code and dependencies also belong to the cell cache identity.
///
/// The per-artifact digests are sorted, so the key is over the SET of artifacts a cell was cut
/// from: handing the same two artifacts in the other order is not a re-cut.
pub fn fingerprint(paths: &[PathBuf]) -> Result<String, String> {
    let mut digests = Vec::new();
    for path in paths {
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let content: Content = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        let root = path.parent().ok_or("landmark content has no directory")?;
        for photo in content.records.iter().filter_map(|record| record.photo.as_ref()) {
            photo_pixels(root, photo)?;
        }
        digests.push(<[u8; 32]>::from(Sha256::digest(&bytes)));
    }
    digests.sort_unstable();
    digests.dedup();
    let mut hash = Sha256::new();
    for digest in digests {
        hash.update(digest);
    }
    hash.update(include_bytes!("landmark_map.rs"));
    hash.update(include_bytes!("../../../firmware/obc-formats/src/obcm/landmarks.rs"));
    hash.update(include_bytes!("../../../firmware/obc-formats/src/articles.rs"));
    hash.update(include_bytes!("../../../Cargo.lock"));
    Ok(hash.finalize().iter().map(|byte| format!("{byte:02x}")).collect())
}

fn photo_pixels(root: &Path, photo: &Photo) -> Result<Vec<u8>, String> {
    if Path::new(&photo.path).components().any(|part| !matches!(part, std::path::Component::Normal(_))) {
        return Err("invalid landmark photo path".into());
    }
    let file = root.join(&photo.path);
    let metadata = fs::symlink_metadata(&file).map_err(|e| e.to_string())?;
    if !metadata.is_file()
        || metadata.len() != PHOTO_PIXELS as u64
        || !file.canonicalize().map_err(|e| e.to_string())?.starts_with(root.canonicalize().map_err(|e| e.to_string())?)
    {
        return Err("invalid landmark photo file".into());
    }
    let pixels = fs::read(file).map_err(|e| e.to_string())?;
    let digest = Sha256::digest(&pixels).iter().map(|b| format!("{b:02x}")).collect::<String>();
    if pixels.len() != PHOTO_PIXELS
        || photo.bytes != PHOTO_PIXELS
        || digest != photo.sha256
        || pixels.iter().any(|&p| p >= 64)
    {
        return Err("invalid landmark photo pixels or digest".into());
    }
    Ok(pixels)
}

fn pages(fields: &[String]) -> Result<Vec<u8>, String> {
    let count = u16::try_from(fields.len()).map_err(|_| "too many landmark fields")?;
    let mut offset = 2 + (fields.len() + 1) * 4;
    let mut bytes = Vec::with_capacity(offset);
    bytes.extend_from_slice(&count.to_le_bytes());
    for field in fields {
        bytes.extend_from_slice(&u32::try_from(offset).map_err(|_| "landmark field overflow")?.to_le_bytes());
        offset = offset.checked_add(field.len()).ok_or("landmark field overflow")?;
    }
    bytes.extend_from_slice(&u32::try_from(offset).map_err(|_| "landmark field overflow")?.to_le_bytes());
    for field in fields {
        bytes.extend_from_slice(field.as_bytes());
    }
    Ok(bytes)
}

fn attribution(source: &Attribution) -> Result<Vec<u8>, String> {
    if source.display_pages.is_empty()
        || source.display_pages.len() > MAX_CREDIT_PAGES as usize
        || source.display_pages.iter().any(|page| page.len() > MAX_PAGE_BYTES)
    {
        return Err("landmark attribution page budget".into());
    }
    let mut fields = vec![
        source.source_url.clone(),
        source.revision.clone(),
        source.license_url.clone(),
        source.original_notices.clone(),
    ];
    fields.extend(source.display_pages.iter().cloned());
    let bytes = pages(&fields)?;
    if bytes.len() > MAX_ATTRIBUTION_BYTES as usize {
        return Err("landmark attribution byte budget".into());
    }
    Ok(bytes)
}

/// Internal references are bundle-relative, so map assembly copies the bundle unchanged.
fn article_bundle(default_language: &str, text_variants: &[crate::landmarks::TextVariant]) -> Result<Vec<u8>, String> {
    use obc_formats::articles::{self, ArticleVariant, LANGUAGES, VARIANT_LEN};
    let default: [u8; 2] = default_language.as_bytes().try_into().map_err(|_| "invalid default language")?;
    if text_variants.is_empty() || text_variants.len() > LANGUAGES.len() {
        return Err("article language count".into());
    }
    let mut variants: Vec<_> = text_variants.iter().collect();
    variants.sort_by_key(|v| LANGUAGES.iter().position(|l| l.as_slice() == v.language.as_bytes()));
    let mut seen = BTreeSet::new();
    let mut bytes = vec![0; articles::HEADER_LEN + variants.len() * VARIANT_LEN];
    bytes[..2].copy_from_slice(&default);
    bytes[2..4].copy_from_slice(&(variants.len() as u16).to_le_bytes());
    for (i, variant) in variants.iter().enumerate() {
        let language: [u8; 2] = variant.language.as_bytes().try_into().map_err(|_| "invalid article language")?;
        if !LANGUAGES.contains(&language)
            || !seen.insert(language)
            || variant.text_pages.iter().any(|p| p.len() > MAX_PAGE_BYTES)
        {
            return Err("invalid or duplicate article language".into());
        }
        let text_pages = u8::try_from(variant.text_pages.len()).map_err(|_| "article page count")?;
        let mut append = |blob: Vec<u8>| {
            let reference = ContentRef { offset: bytes.len() as u32, len: blob.len() as u32 };
            bytes.extend(blob);
            reference
        };
        let encoded = ArticleVariant {
            language,
            text_pages,
            text: append(pages(&variant.text_pages)?),
            attribution: append(attribution(&variant.attribution)?),
        };
        if ArticleVariant::decode(&encoded.encode()).is_none() {
            return Err("article page count".into());
        }
        let at = articles::HEADER_LEN + i * VARIANT_LEN;
        bytes[at..at + VARIANT_LEN].copy_from_slice(&encoded.encode());
    }
    if !seen.contains(&default) || bytes.len() > articles::MAX_BYTES as usize {
        return Err("invalid article default or size".into());
    }
    Ok(bytes)
}

fn article_link(record: &crate::landmarks::TextVariant) -> Option<String> {
    let url = url::Url::parse(&record.attribution.source_url).ok()?;
    let expected = format!("{}.wikipedia.org", record.language);
    if url.host_str()? != expected {
        return None;
    }
    let title = url.query_pairs().find(|(key, _)| key == "title")?.1.into_owned();
    Some(format!("{}:{}", record.language, title.replace('_', " ")))
}

/// Inputs are one or more compiled artifacts: a content.json and its sibling digest-pinned pixel
/// files. The chosen OSM link is deterministic: available approach first, then source identity.
///
/// Region boundaries meet and overlap, so a cell can be handed two artifacts that both carry a
/// place. One record per QID reaches the map, and which one is decided by [`rank`], never by the
/// order the artifacts arrive in.
pub fn load(paths: &[PathBuf], links: &[LandmarkLink], bbox: (i64, i64, i64, i64)) -> Result<Vec<Landmark>, String> {
    let mut merged: BTreeMap<u64, Landmark> = BTreeMap::new();
    for path in paths {
        let content: Content =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if content.schema != 2 {
            return Err("unsupported landmark content schema".into());
        }
        let root = path.parent().ok_or("landmark content has no directory")?;
        let mut qids = BTreeSet::new();
        for record in content.records {
            let qid = record
                .qid
                .strip_prefix('Q')
                .and_then(|id| id.parse::<u64>().ok())
                .filter(|id| *id > 0)
                .ok_or("invalid landmark QID")?;
            if !qids.insert(qid) {
                return Err("duplicate landmark QID".into());
            }
            if !record.longitude.is_finite()
                || !record.latitude.is_finite()
                || !(-180.0..=180.0).contains(&record.longitude)
                || !(-90.0..=90.0).contains(&record.latitude)
            {
                return Err("invalid landmark coordinate".into());
            }
            let lon = (record.longitude * 1_000_000.0).round_ties_even() as i32;
            let lat = (record.latitude * 1_000_000.0).round_ties_even() as i32;
            if i64::from(lon) < bbox.0 || i64::from(lat) < bbox.1 || i64::from(lon) > bbox.2 || i64::from(lat) > bbox.3
            {
                continue;
            }
            let articles: Vec<_> = record.variants.iter().filter_map(article_link).collect();
            let link = links
                .iter()
                .filter(|link| {
                    link.wikidata.as_deref() == Some(record.qid.as_str())
                        || (link.wikidata.is_none()
                            && link.wikipedia.as_ref().is_some_and(|b| articles.contains(&b.replace('_', " "))))
                })
                .min_by_key(|link| (link.metadata.approach.is_none(), link.metadata.source));
            if record.name.is_empty() || record.name.len() > MAX_NAME_BYTES as usize {
                return Err("landmark name budget".into());
            }
            let blobs =
                encode_content(root, &record.name, &record.default_language, &record.variants, record.photo.as_ref())?;
            let mut encoded = LandmarkRecord {
                qid,
                lon,
                lat,
                category: record.category,
                hours_ref: POI_HOURS_REF_NONE,
                osm: link.map(|link| link.metadata),
                name: ContentRef::default(),
                articles: ContentRef::default(),
                photo: ContentRef::default(),
                photo_attribution: ContentRef::default(),
            };
            if LandmarkRecord::decode(&encoded.encode()).is_none() {
                return Err("invalid landmark metadata".into());
            }
            // Offsets are assigned only when this cell's content pool is known.
            encoded.hours_ref = POI_HOURS_REF_NONE;
            let landmark =
                Landmark { record: encoded, hours: link.and_then(|link| link.hours.clone()), content: blobs };
            match merged.entry(qid) {
                Entry::Vacant(slot) => {
                    slot.insert(landmark);
                }
                Entry::Occupied(mut slot) => {
                    if rank(&landmark) < rank(slot.get()) {
                        slot.insert(landmark);
                    }
                }
            }
        }
    }
    let mut output: Vec<Landmark> = merged.into_values().collect();
    output.sort_by_key(|landmark| landmark.record.key());
    if output.len() > MAX_RECORDS as usize {
        return Err("landmark record budget".into());
    }
    Ok(output)
}

/// Which of two records for one QID is kept: the lower encoding wins.
///
/// Every byte that reaches the card is in here — the metadata and a digest per content blob — so
/// two artifacts that agree on a place agree on the record, and two that disagree resolve the same
/// way in every cell, in every run and for any order of the artifacts. It is the rule
/// [`crate::peak_map::load`] applies to a duplicate peak article.
fn rank(landmark: &Landmark) -> ([u8; RECORD_LEN], [[u8; 32]; 4]) {
    (landmark.record.encode(), landmark.content.each_ref().map(|blob| <[u8; 32]>::from(Sha256::digest(blob))))
}

pub(crate) fn encode_content(
    root: &Path,
    name: &str,
    default: &str,
    variants: &[crate::landmarks::TextVariant],
    photo: Option<&Photo>,
) -> Result<[Vec<u8>; 4], String> {
    if name.is_empty() || name.len() > MAX_NAME_BYTES as usize {
        return Err("article name budget".into());
    }
    let bundle = article_bundle(default, variants)?;
    let mut blobs = [name.as_bytes().to_vec(), bundle, Vec::new(), Vec::new()];
    if let Some(photo) = photo {
        let pixels = photo_pixels(root, photo)?;
        let mut buffer = vec![0; zlib_rs::compress_bound(pixels.len())];
        let (stream, code) = zlib_rs::compress_slice(
            &mut buffer,
            &pixels,
            zlib_rs::DeflateConfig { level: 9, window_bits: i32::from(PHOTO_WINDOW_BITS), ..Default::default() },
        );
        if code != zlib_rs::ReturnCode::Ok || stream.len() > PHOTO_MAX_COMPRESSED {
            return Err("landmark compression failed".into());
        }
        blobs[2] = stream.to_vec();
        blobs[3] = attribution(&photo.attribution)?;
    }
    Ok(blobs)
}

pub fn serialize(landmarks: &[Landmark], hours_refs: &[u16]) -> Result<Vec<u8>, String> {
    if landmarks.is_empty() {
        return Ok(Vec::new());
    }
    if landmarks.len() > MAX_RECORDS as usize || landmarks.len() != hours_refs.len() {
        return Err("landmark record budget".into());
    }
    let payload = SECTION_HEADER_LEN + landmarks.len() * RECORD_LEN;
    let mut bytes = vec![0; payload];
    let mut pool: BTreeMap<&[u8], ContentRef> = BTreeMap::new();
    let mut qids = BTreeSet::new();
    let mut previous = None;
    for (index, (landmark, &hours_ref)) in landmarks.iter().zip(hours_refs).enumerate() {
        let mut record = landmark.record;
        record.hours_ref = hours_ref;
        if !qids.insert(record.qid) || previous.is_some_and(|key| key >= record.key()) {
            return Err("landmark records must be ordered with unique QIDs".into());
        }
        previous = Some(record.key());
        if LandmarkRecord::decode(&record.encode()).is_none()
            || landmark.content[..2].iter().any(Vec::is_empty)
            || landmark.content[2].is_empty() != landmark.content[3].is_empty()
        {
            return Err("invalid landmark metadata or content".into());
        }
        let mut refs = [ContentRef::default(); 4];
        for (slot, blob) in refs.iter_mut().zip(&landmark.content) {
            if blob.is_empty() {
                continue;
            }
            *slot = if let Some(reference) = pool.get(blob.as_slice()) {
                *reference
            } else {
                let reference = ContentRef {
                    offset: u32::try_from(bytes.len()).map_err(|_| "landmark section overflow")?,
                    len: u32::try_from(blob.len()).map_err(|_| "landmark content overflow")?,
                };
                bytes.extend_from_slice(blob);
                pool.insert(blob, reference);
                reference
            };
        }
        [record.name, record.articles, record.photo, record.photo_attribution] = refs;
        let at = SECTION_HEADER_LEN + index * RECORD_LEN;
        bytes[at..at + RECORD_LEN].copy_from_slice(&record.encode());
    }
    let len = u32::try_from(bytes.len()).map_err(|_| "landmark section overflow")?;
    bytes[..4].copy_from_slice(&(landmarks.len() as u32).to_le_bytes());
    bytes[4..6].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
    bytes[6..8].copy_from_slice(&SECTION_VERSION.to_le_bytes());
    bytes[8..12].copy_from_slice(&(payload as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&len.to_le_bytes());
    Ok(bytes)
}
