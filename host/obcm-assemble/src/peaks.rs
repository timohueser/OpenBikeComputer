//! Source-backed peak collection merge, selected by the summit identities in the output.
use crate::{
    emit::MapWriter,
    input::Cell,
    landmarks::{malformed, Blob},
    poi::MergedPois,
    Error, Result,
};
use obc_formats::obcm::peaks::{HEADER_LEN, MAX_RECORDS, RECORD_LEN, VERSION};
use obc_formats::{
    io::{ByteSource, WindowSource},
    obcm::{landmarks::*, peaks::*, SourceId, SUMMIT_SUBTYPE_ID},
};
use obc_reader::peaks::{map_section, Directory};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Default)]
pub struct PeakSection<'a> {
    associations: Vec<Association>,
    records: Vec<Record>,
    blobs: Vec<Blob>,
    len: u32,
    /// The input sections that the blobs are in.
    sections: Vec<WindowSource<'a>>,
}
impl PeakSection<'_> {
    pub fn section_len(&self) -> u64 {
        if self.associations.is_empty() {
            0
        } else {
            crate::emit::align_up(u64::from(self.len))
        }
    }
    pub fn emit(&self, out: &mut MapWriter<'_>) -> Result<()> {
        if self.associations.is_empty() {
            return Ok(());
        }
        let mut h = [0; HEADER_LEN];
        h[..2].copy_from_slice(&VERSION.to_le_bytes());
        h[2..4].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
        h[4..8].copy_from_slice(&(self.associations.len() as u32).to_le_bytes());
        h[8..12].copy_from_slice(&(self.records.len() as u32).to_le_bytes());
        let payload = HEADER_LEN + self.associations.len() * ASSOCIATION_LEN + self.records.len() * RECORD_LEN;
        h[12..16].copy_from_slice(&(payload as u32).to_le_bytes());
        h[16..20].copy_from_slice(&self.len.to_le_bytes());
        out.put(&h)?;
        for a in &self.associations {
            out.put(&a.encode())?;
        }
        for r in &self.records {
            out.put(&r.encode())?;
        }
        for b in &self.blobs {
            let mut h = Sha256::new();
            b.copy(&self.sections, |bytes| {
                h.update(bytes);
                out.put(bytes)
            })?;
            if <[u8; 32]>::from(h.finalize()) != b.hash {
                return Err(Error::Verify("peak source content changed during assembly".into()));
            }
        }
        out.begin_section()?;
        Ok(())
    }
}
/// The peak sections of `cells` and the peak `artifacts` (OBCC §14.3), selected by the summits of
/// the merged POIs.
pub fn merge<'a>(cells: &[&Cell<'a>], artifacts: &[&'a dyn ByteSource], pois: &MergedPois) -> Result<PeakSection<'a>> {
    let summits: BTreeSet<_> =
        pois.pois.iter().filter(|p| p.subtype == SUMMIT_SUBTYPE_ID).map(|p| p.metadata.source).collect();
    let mut associations = BTreeMap::<SourceId, ArticleId>::new();
    let mut records = BTreeMap::<ArticleId, [Option<Blob>; 4]>::new();
    let mut sections = Vec::new();
    for cell in cells {
        sections.extend(map_section(cell.src).map_err(malformed)?);
    }
    for &src in artifacts {
        sections.push(WindowSource::new(src, 0, src.len()).expect("a source is a window onto itself"));
    }
    for (index, section) in sections.iter().enumerate() {
        let d = Directory::read(section).map_err(malformed)?;
        let mut wanted = BTreeSet::new();
        let mut last = None;
        for i in 0..d.associations {
            let a = d.association(section, i).map_err(malformed)?;
            if last.is_some_and(|s| s >= a.source) || d.record(section, a.index).map_err(malformed)?.id != a.article {
                return Err(Error::Format("invalid peak association index".into()));
            }
            last = Some(a.source);
            if summits.contains(&a.source) {
                if associations.insert(a.source, a.article).is_some_and(|old| old != a.article) {
                    return Err(Error::Format("conflicting summit article association".into()));
                }
                wanted.insert(a.article);
            }
        }
        let mut last = None;
        for i in 0..d.records {
            let r = d.record(section, i).map_err(malformed)?;
            if last.is_some_and(|id| id >= r.id) {
                return Err(Error::Format("unordered peak articles".into()));
            }
            last = Some(r.id);
            if r.content[2].is_absent() != r.content[3].is_absent() {
                return Err(Error::Format("unpaired peak photo".into()));
            }
            if r.content[1].is_absent() && r.content[2].is_absent() {
                return Err(Error::Format("peak article with neither text nor a photo".into()));
            }
            if !r.content[1].is_absent() {
                let bundle = d.content(section, &r, 1, obc_formats::articles::MAX_BYTES).map_err(malformed)?;
                obc_reader::articles::select(&bundle, *b"en").map_err(malformed)?;
            }
            let limits =
                [MAX_NAME_BYTES, obc_formats::articles::MAX_BYTES, PHOTO_MAX_COMPRESSED as u32, MAX_ATTRIBUTION_BYTES];
            let mut blobs: [Option<Blob>; 4] = std::array::from_fn(|_| None);
            for (j, reference) in r.content.into_iter().enumerate() {
                if j >= 1 && reference.is_absent() {
                    continue;
                }
                d.content(section, &r, j, limits[j]).map_err(malformed)?;
                let mut b =
                    Blob { section: index, offset: u64::from(reference.offset), len: reference.len, hash: [0; 32] };
                let mut h = Sha256::new();
                b.copy(&sections, |bytes| {
                    h.update(bytes);
                    Ok(())
                })?;
                b.hash = h.finalize().into();
                blobs[j] = Some(b);
            }
            if wanted.contains(&r.id) {
                let key = |b: &[Option<Blob>; 4]| {
                    b.each_ref().map(|b| b.as_ref().map_or_else(|| Sha256::digest([]).into(), |b| b.hash))
                };
                records
                    .entry(r.id)
                    .and_modify(|old| {
                        if key(&blobs) < key(old) {
                            *old = blobs.clone();
                        }
                    })
                    .or_insert(blobs);
            }
        }
        if associations.len() > MAX_ASSOCIATIONS as usize || records.len() > MAX_RECORDS as usize {
            return Err(Error::Capacity("peak collection budget".into()));
        }
    }
    if associations.is_empty() {
        return Ok(PeakSection::default());
    }
    let indexes: BTreeMap<_, _> = records.keys().enumerate().map(|(i, &id)| (id, i as u32)).collect();
    let associations: Vec<_> = associations
        .into_iter()
        .map(|(source, article)| Association { source, article, index: indexes[&article] })
        .collect();
    let mut out = PeakSection {
        len: (HEADER_LEN + associations.len() * ASSOCIATION_LEN + records.len() * RECORD_LEN) as u32,
        associations,
        ..Default::default()
    };
    let mut pool = BTreeMap::<[u8; 32], Vec<(usize, ContentRef)>>::new();
    for (id, blobs) in records {
        let mut content = [ContentRef::default(); 4];
        for (j, b) in blobs.into_iter().enumerate() {
            let Some(b) = b else {
                continue;
            };
            let bucket = pool.entry(b.hash).or_default();
            let mut existing = None;
            for &(i, r) in bucket.iter() {
                if b.same_bytes(&out.blobs[i], &sections)? {
                    existing = Some(r);
                    break;
                }
            }
            content[j] = if let Some(r) = existing {
                r
            } else {
                let r = ContentRef { offset: out.len, len: b.len };
                out.len = out.len.checked_add(b.len).ok_or_else(|| Error::Capacity("peak section overflow".into()))?;
                bucket.push((out.blobs.len(), r));
                out.blobs.push(b);
                r
            };
        }
        out.records.push(Record { id, content });
    }
    out.sections = sections;
    Ok(out)
}
