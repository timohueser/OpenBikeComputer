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
