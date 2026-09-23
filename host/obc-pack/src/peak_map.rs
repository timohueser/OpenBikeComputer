//! Join canonical peak articles to the exact summit nodes carried by a map.
use crate::{landmarks::peaks::PeakContent, poi::Poi};
use obc_formats::obcm::{landmarks::ContentRef, peaks::*, SourceId, SUMMIT_SUBTYPE_ID};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Default)]
pub struct Peaks {
    pub associations: BTreeMap<SourceId, ArticleId>,
    pub records: BTreeMap<ArticleId, [Vec<u8>; 4]>,
}
impl Peaks {
    pub fn select(&self, pois: &[Poi]) -> Self {
        let associations: BTreeMap<_, _> = pois
            .iter()
            .filter(|p| p.subtype == SUMMIT_SUBTYPE_ID)
            .filter_map(|p| self.associations.get(&p.metadata.source).map(|&a| (p.metadata.source, a)))
            .collect();
        let wanted: BTreeSet<_> = associations.values().collect();
        Self {
            records: self
                .records
                .iter()
                .filter(|(id, _)| wanted.contains(id))
                .map(|(id, r)| (*id, r.clone()))
                .collect(),
            associations,
        }
    }
}
/// Include bytes and encoder policy in each cell's cache identity.
pub fn fingerprint(paths: &[PathBuf]) -> Result<String, String> {
    let mut digests = Vec::new();
    for path in paths {
        let bytes = fs::read(path).map_err(|e| e.to_string())?;
        let content: PeakContent = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        // Loading checks declared photo bytes as well as the source manifest.
        load(std::slice::from_ref(path))?;
        let mut h = Sha256::new();
        h.update(bytes);
        for p in content.records.iter().filter_map(|r| r.article.photo.as_ref()) {
            h.update(&p.sha256);
        }
        digests.push(<[u8; 32]>::from(h.finalize()));
    }
    digests.sort();
    digests.dedup();
    let mut h = Sha256::new();
    for d in digests {
        h.update(d);
    }
    h.update(include_bytes!("peak_map.rs"));
    h.update(include_bytes!("landmark_map.rs"));
    h.update(include_bytes!("landmarks/credit.rs"));
    h.update(include_bytes!("../../../firmware/obc-formats/src/obcm/peaks.rs"));
    h.update(include_bytes!("../../../firmware/obc-formats/src/articles.rs"));
    h.update(include_bytes!("../../../Cargo.lock"));
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
/// Duplicate articles use canonical encoded-byte order. Conflicting summit links fail closed.
pub fn load(paths: &[PathBuf]) -> Result<Peaks, String> {
    let mut out = Peaks::default();
    let mut identities = BTreeMap::new();
    for path in paths {
        let c: PeakContent =
            serde_json::from_slice(&fs::read(path).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        if c.schema != 1 || c.collection != "peaks" {
            return Err("unsupported peak collection".into());
        }
        let root = path.parent().unwrap_or(Path::new("."));
        let mut local = BTreeMap::new();
        for r in c.records {
            if r.id.is_empty() {
                return Err("empty peak article identity".into());
            }
            let id: ArticleId = Sha256::digest(r.id.as_bytes()).into();
            if identities.insert(id, r.id.clone()).is_some_and(|old| old != r.id) {
                return Err("peak identity digest collision".into());
            }
            if local.insert(r.id, id).is_some() {
                return Err("duplicate peak article".into());
            }
            let a = r.article;
            let mut blobs =
                crate::landmark_map::encode_content(root, &a.name, &a.default_language, &a.variants, a.photo.as_ref())?;
            if blobs[1].is_empty() && blobs[2].is_empty() {
                return Err("peak article with neither text nor a photo".into());
            }
            for (slot, blob) in blobs.iter_mut().enumerate() {
                if !blob.is_empty() {
                    let mut guarded = id.to_vec();
                    guarded.push(slot as u8);
                    guarded.append(blob);
                    *blob = guarded;
                }
            }
            out.records
                .entry(id)
                .and_modify(|old| {
                    if blobs.each_ref().map(|b| <[u8; 32]>::from(Sha256::digest(b)))
                        < old.each_ref().map(|b| <[u8; 32]>::from(Sha256::digest(b)))
                    {
                        *old = blobs.clone();
                    }
                })
                .or_insert(blobs);
        }
        let mut seen = BTreeSet::new();
        for a in c.associations {
            let source = SourceId::osm(1, u64::try_from(a.node_id).map_err(|_| "invalid summit node")?);
            if !source.is_valid() || !seen.insert(source) {
                return Err("invalid or duplicate summit node".into());
            }
            let id = *local.get(&a.article_id).ok_or("missing peak article")?;
            if out.associations.insert(source, id).is_some_and(|old| old != id) {
                return Err("conflicting summit article association".into());
            }
        }
    }
    if out.records.len() > MAX_RECORDS as usize || out.associations.len() > MAX_ASSOCIATIONS as usize {
        return Err("peak collection budget".into());
    }
    Ok(out)
}
pub fn serialize(peaks: &Peaks) -> Result<Vec<u8>, String> {
    if peaks.associations.is_empty() {
        return Ok(Vec::new());
    }
    if peaks.records.len() > MAX_RECORDS as usize || peaks.associations.len() > MAX_ASSOCIATIONS as usize {
        return Err("peak collection budget".into());
    }
    let payload = HEADER_LEN + peaks.associations.len() * ASSOCIATION_LEN + peaks.records.len() * RECORD_LEN;
    let mut bytes = vec![0; payload];
    bytes[..2].copy_from_slice(&VERSION.to_le_bytes());
    bytes[2..4].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
    bytes[4..8].copy_from_slice(&(peaks.associations.len() as u32).to_le_bytes());
    bytes[8..12].copy_from_slice(&(peaks.records.len() as u32).to_le_bytes());
    bytes[12..16].copy_from_slice(&(payload as u32).to_le_bytes());
    let indexes: BTreeMap<_, _> = peaks.records.keys().enumerate().map(|(i, &id)| (id, i as u32)).collect();
    for (i, (&source, &article)) in peaks.associations.iter().enumerate() {
        let a = Association { source, article, index: *indexes.get(&article).ok_or("missing peak article")? };
        let at = HEADER_LEN + i * ASSOCIATION_LEN;
        bytes[at..at + ASSOCIATION_LEN].copy_from_slice(&a.encode());
    }
    let mut pool = BTreeMap::<&[u8], ContentRef>::new();
    for (i, (&id, blobs)) in peaks.records.iter().enumerate() {
        let mut content = [ContentRef::default(); 4];
        for (r, b) in content.iter_mut().zip(blobs) {
            if b.is_empty() {
                continue;
            }
            *r = if let Some(r) = pool.get(b.as_slice()) {
                *r
            } else {
                let r = ContentRef {
                    offset: u32::try_from(bytes.len()).map_err(|_| "peak section overflow")?,
                    len: u32::try_from(b.len()).map_err(|_| "peak blob overflow")?,
                };
                bytes.extend(b);
                pool.insert(b, r);
                r
            };
        }
        let at = HEADER_LEN + peaks.associations.len() * ASSOCIATION_LEN + i * RECORD_LEN;
        bytes[at..at + RECORD_LEN].copy_from_slice(&Record { id, content }.encode());
    }
    let len = u32::try_from(bytes.len()).map_err(|_| "peak section overflow")?;
    bytes[16..20].copy_from_slice(&len.to_le_bytes());
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::{
        io::{ByteSource, SliceSource},
        obcm::landmarks::*,
    };
    use obc_reader::{
        peaks::Directory,
        photo::{PhotoDecoder, Progress},
    };
    use serde_json::json;
    #[test]
    fn regional_catalogues_deduplicate_photos_and_reject_conflicting_or_missing_links() {
        let root = obcm_testkit::scratch::scratch_dir("peak-map", "sources");
        let pixels = vec![7; PHOTO_PIXELS];
        fs::write(root.join("photo.rgb222"), &pixels).unwrap();
        let digest: String = Sha256::digest(&pixels).iter().map(|b| format!("{b:02x}")).collect();
        let credit = json!({"source_url":"https://en.wikipedia.org/w/index.php?title=Mountain&oldid=1","revision":"1","license_url":"https://creativecommons.org/licenses/by-sa/4.0/","original_notices":"Authors"});
        let photo_credit = json!({"source_url":"https://commons.wikimedia.org/wiki/File:Mountain.jpg","revision":"1","license_url":"https://creativecommons.org/licenses/by/4.0/","original_notices":r#"{"Artist":{"value":"A"}}"#});
        let mut catalogue = json!({"schema":1,"collection":"peaks","input_sha256":"input","policy_sha256":"policy","languages":["en","de","fr","es"],"source_coverage":{},"counts":crate::landmarks::Counts::default(),"omissions":[],"records":[{"id":"Q7","name":"Mountain","default_language":"en","fallback_sources":[],"variants":[{"language":"en","text_pages":["A mountain."],"attribution":credit}],"photo":{"path":"photo.rgb222","bytes":PHOTO_PIXELS,"sha256":digest,"attribution":photo_credit}}],"associations":[{"node_id":101,"article_id":"Q7","latitude":-80,"longitude":-160},{"node_id":102,"article_id":"Q7","latitude":80,"longitude":160}]});
        let a = root.join("a.json");
        fs::write(&a, serde_json::to_vec(&catalogue).unwrap()).unwrap();
        catalogue["records"][0]["variants"][0]["text_pages"][0] = json!("Another captured revision.");
        let b = root.join("b.json");
        fs::write(&b, serde_json::to_vec(&catalogue).unwrap()).unwrap();
        let forward = load(&[a.clone(), b.clone()]).unwrap();
        let reverse = load(&[b.clone(), a.clone()]).unwrap();
        let bytes = serialize(&forward).unwrap();
        assert_eq!(bytes, serialize(&reverse).unwrap());
        let src = SliceSource(&bytes);
        let d = Directory::read(&src).unwrap();
        assert_eq!((d.records, d.associations), (1, 2));
        let record = d.record(&src, 0).unwrap();
        let photo = d.content(&src, &record, 2, PHOTO_MAX_COMPRESSED as u32).unwrap();
        let mut decoder = PhotoDecoder::new();
        let mut decoded = vec![];
        for _ in 0..1024 {
            if decoder.step(&photo, |_, b| decoded.extend_from_slice(b)).unwrap() == Progress::Complete {
                break;
            }
        }
        assert_eq!(decoded, pixels);
        let id = digest_id("Q7");
        assert_eq!(forward.associations.get(&SourceId::osm(1, 102)), Some(&id));
        let key = fingerprint(&[a.clone(), b.clone()]).unwrap();
        assert_eq!(key, fingerprint(&[b.clone(), a.clone()]).unwrap());
        catalogue["associations"][0]["article_id"] = json!("Q8");
        fs::write(&b, serde_json::to_vec(&catalogue).unwrap()).unwrap();
        assert!(load(std::slice::from_ref(&b)).is_err());
        catalogue["records"][0]["id"] = json!("Q8");
        catalogue["associations"][1]["article_id"] = json!("Q8");
        fs::write(&b, serde_json::to_vec(&catalogue).unwrap()).unwrap();
        assert!(load(&[a.clone(), b.clone()]).is_err());
        fs::write(root.join("photo.rgb222"), vec![0; PHOTO_PIXELS]).unwrap();
        assert!(fingerprint(&[a]).is_err());
        assert!(photo.len() < (PHOTO_PIXELS as u64));
    }
    fn digest_id(id: &str) -> ArticleId {
        Sha256::digest(id.as_bytes()).into()
    }
    #[test]
    fn a_record_with_a_photo_and_no_text_writes_an_absent_bundle() {
        let root = obcm_testkit::scratch::scratch_dir("peak-map", "photo-only");
        let pixels = vec![9; PHOTO_PIXELS];
        fs::write(root.join("photo.rgb222"), &pixels).unwrap();
        let digest: String = Sha256::digest(&pixels).iter().map(|b| format!("{b:02x}")).collect();
        let credit = json!({"source_url":"https://commons.wikimedia.org/wiki/File:Peak.png","revision":"1","license_url":"https://creativecommons.org/licenses/by/4.0/","original_notices":"{\"Artist\":{\"value\":\"Example\"}}"});
        let mut catalogue = json!({"schema":1,"collection":"peaks","input_sha256":"input","policy_sha256":"policy","languages":["en","de","fr","es"],"source_coverage":{},"counts":crate::landmarks::Counts::default(),"omissions":[],
            "records":[{"id":"Q7","name":"Schafberg","default_language":"","fallback_sources":[],"variants":[],
            "photo":{"path":"photo.rgb222","bytes":PHOTO_PIXELS,"sha256":digest,"attribution":credit}}],
            "associations":[{"node_id":101,"article_id":"Q7","latitude":0,"longitude":0}]});
        let path = root.join("photo-only.json");
        fs::write(&path, serde_json::to_vec(&catalogue).unwrap()).unwrap();
        let bytes = serialize(&load(std::slice::from_ref(&path)).unwrap()).unwrap();
        let src = SliceSource(&bytes);
        let d = Directory::read(&src).unwrap();
        let record = d.record(&src, 0).unwrap();
        assert!(record.content[1].is_absent(), "no text means no article bundle");
        assert!(d.content(&src, &record, 0, MAX_NAME_BYTES).is_ok());
        assert!(d.content(&src, &record, 2, PHOTO_MAX_COMPRESSED as u32).is_ok());
        catalogue["records"][0]["photo"] = json!(null);
        fs::write(&path, serde_json::to_vec(&catalogue).unwrap()).unwrap();
        assert!(load(std::slice::from_ref(&path)).is_err(), "a record with neither text nor a photo is refused");
    }
}
