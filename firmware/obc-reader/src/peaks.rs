//! Direct identity lookup for Peak View, independent of landmark queries.
use crate::{Error, Reader};
use obc_formats::obcm::peaks::{HEADER_LEN, MAX_RECORDS, RECORD_LEN, VERSION};
use obc_formats::{
    io::{rd_u16, rd_u32, ByteSource, WindowSource},
    obcm::{self, peaks::*},
};

pub fn map_section(source: &dyn ByteSource) -> Result<Option<WindowSource<'_>>, Error> {
    let mut header = [0; obcm::HEADER_LEN];
    source.read_at(0, &mut header).map_err(Error::Source)?;
    if header[..4] != obcm::MAGIC {
        return Err(Error::BadMagic);
    }
    if header[4] != obcm::VERSION {
        return Err(Error::BadVersion);
    }
    let scale = obcm::OffsetScale::new(header[obcm::HEADER_OFFSET_SCALE_OFF]).map_err(|_| Error::BadScale)?;
    let start = scale.offset(rd_u32(&header, obcm::HEADER_PEAK_OFFSET_OFF)).bytes();
    let len = scale.offset(rd_u32(&header, obcm::HEADER_PEAK_LENGTH_OFF)).bytes();
    if start == 0 && len == 0 {
        return Ok(None);
    }
    if start < obcm::HEADER_LEN as u64 || len < HEADER_LEN as u64 {
        return Err(Error::BadOffset);
    }
    WindowSource::new(source, start, len).map(Some).ok_or(Error::BadOffset)
}
#[derive(Clone, Copy, Debug)]
pub struct Directory {
    pub associations: u32,
    pub records: u32,
    pub payload: u32,
    pub len: u32,
}
impl Directory {
    pub fn read(source: &dyn ByteSource) -> Result<Self, Error> {
        let mut h = [0; HEADER_LEN];
        source.read_at(0, &mut h).map_err(Error::Source)?;
        let d =
            Self { associations: rd_u32(&h, 4), records: rd_u32(&h, 8), payload: rd_u32(&h, 12), len: rd_u32(&h, 16) };
        if rd_u16(&h, 0) != VERSION {
            return Err(Error::BadVersion);
        }
        if rd_u16(&h, 2) as usize != RECORD_LEN
            || rd_u32(&h, 20) != 0
            || d.records > MAX_RECORDS
            || d.associations > MAX_ASSOCIATIONS
            || d.payload != HEADER_LEN as u32 + d.associations * ASSOCIATION_LEN as u32 + d.records * RECORD_LEN as u32
            || d.len < d.payload
            || u64::from(d.len) > source.len()
        {
            return Err(Error::BadOffset);
        }
        Ok(d)
    }
    pub fn association(&self, source: &dyn ByteSource, index: u32) -> Result<Association, Error> {
        if index >= self.associations {
            return Err(Error::BadOffset);
        }
        let mut b = [0; ASSOCIATION_LEN];
        source.read_at(HEADER_LEN as u64 + u64::from(index) * ASSOCIATION_LEN as u64, &mut b).map_err(Error::Source)?;
        Association::decode(&b).ok_or(Error::BadOffset)
    }
    pub fn record(&self, source: &dyn ByteSource, index: u32) -> Result<Record, Error> {
        if index >= self.records {
            return Err(Error::BadOffset);
        }
        let mut b = [0; RECORD_LEN];
        source
            .read_at(
                HEADER_LEN as u64
                    + u64::from(self.associations) * ASSOCIATION_LEN as u64
                    + u64::from(index) * RECORD_LEN as u64,
                &mut b,
            )
            .map_err(Error::Source)?;
        Ok(Record::decode(&b))
    }
    pub fn content<'a>(
        &self,
        source: &'a dyn ByteSource,
        record: &Record,
        slot: usize,
        limit: u32,
    ) -> Result<WindowSource<'a>, Error> {
        let reference = *record.content.get(slot).ok_or(Error::BadOffset)?;
        let r = reference.range(self.payload, self.len, limit + CONTENT_GUARD_LEN).ok_or(Error::BadOffset)?;
        if reference.len <= CONTENT_GUARD_LEN {
            return Err(Error::BadOffset);
        }
        let mut guard = [0; CONTENT_GUARD_LEN as usize];
        source.read_at(r.start, &mut guard).map_err(Error::Source)?;
        if guard[..32] != record.id || usize::from(guard[32]) != slot {
            return Err(Error::BadOffset);
        }
        WindowSource::new(
            source,
            r.start + u64::from(CONTENT_GUARD_LEN),
            r.end - r.start - u64::from(CONTENT_GUARD_LEN),
        )
        .ok_or(Error::BadOffset)
    }
    pub fn find(&self, source: &dyn ByteSource, identity: obcm::SourceId) -> Result<Option<Association>, Error> {
        let (mut lo, mut hi) = (0, self.associations);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let a = self.association(source, mid)?;
            if a.source < identity {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        if lo == self.associations {
            return Ok(None);
        }
        let a = self.association(source, lo)?;
        if a.source != identity {
            return Ok(None);
        }
        if self.record(source, a.index)?.id != a.article {
            return Err(Error::BadOffset);
        }
        Ok(Some(a))
    }
}
/// Opaque selection tied to one mounted map and one exact summit association.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    generation: u32,
    association: Association,
}

impl Selection {
    /// The [`Reader::generation`] this selection was made on.
    pub fn generation(&self) -> u32 {
        self.generation
    }
}

impl Reader<'_> {
    pub fn peak_article(&self, source: obcm::SourceId) -> Result<Option<Selection>, Error> {
        let Some(section) = map_section(self.source())? else {
            return Ok(None);
        };
        let directory = Directory::read(&section)?;
        let Some(association) = directory.find(&section, source)? else {
            return Ok(None);
        };
        let record = directory.record(&section, association.index)?;
        // Content is text or a photo. A record with neither is one Peak View must not mark.
        if record.content[1].is_absent() {
            directory.content(&section, &record, 2, obcm::landmarks::PHOTO_MAX_COMPRESSED as u32)?;
        } else {
            crate::articles::select(
                &directory.content(&section, &record, 1, obc_formats::articles::MAX_BYTES)?,
                *b"en",
            )?;
        }
        Ok(Some(Selection { generation: self.generation(), association }))
    }
    /// Revalidates the generation and identity before exposing bounded content windows.
    pub fn with_peak_article<T>(
        &self,
        selection: Selection,
        read: impl FnOnce(&WindowSource<'_>, Directory, Record) -> Result<T, Error>,
    ) -> Result<T, Error> {
        if selection.generation != self.generation() {
            return Err(Error::BadOffset);
        }
        let section = map_section(self.source())?.ok_or(Error::BadOffset)?;
        let directory = Directory::read(&section)?;
        if directory.find(&section, selection.association.source)? != Some(selection.association) {
            return Err(Error::BadOffset);
        }
        let record = directory.record(&section, selection.association.index)?;
        read(&section, directory, record)
    }
}
