//! Selected landmark content and incremental nearby pages from one map section.

use crate::Error;
use heapless::Vec;
use obc_formats::{
    io::{rd_u16, rd_u32, ByteSource, WindowSource},
    obcm::{
        landmarks::*, OffsetScale, HEADER_LANDMARK_LENGTH_OFF, HEADER_LANDMARK_OFFSET_OFF, HEADER_LEN,
        HEADER_OFFSET_SCALE_OFF, MAGIC, VERSION,
    },
};
use obc_map_scene::{cos_lat, ground_dist_m_cl};

pub(crate) fn map_region(header: &[u8; HEADER_LEN], total: u64) -> Result<Option<core::ops::Range<u64>>, Error> {
    let scale = OffsetScale::new(header[HEADER_OFFSET_SCALE_OFF]).map_err(|_| Error::BadScale)?;
    let start = scale.offset(rd_u32(header, HEADER_LANDMARK_OFFSET_OFF)).bytes();
    let len = scale.offset(rd_u32(header, HEADER_LANDMARK_LENGTH_OFF)).bytes();
    if start == 0 && len == 0 {
        return Ok(None);
    }
    let end = start.checked_add(len).ok_or(Error::BadOffset)?;
    if start < HEADER_LEN as u64 || len < SECTION_HEADER_LEN as u64 || end > total {
        return Err(Error::BadOffset);
    }
    Ok(Some(start..end))
}

/// Resolve the optional section through the same byte seam as ordinary maps.
pub fn map_section(source: &dyn ByteSource) -> Result<Option<WindowSource<'_>>, Error> {
    let mut header = [0; HEADER_LEN];
    source.read_at(0, &mut header).map_err(Error::Source)?;
    if header[..4] != MAGIC {
        return Err(Error::BadMagic);
    }
    if header[4] != VERSION {
        return Err(Error::BadVersion);
    }
    map_region(&header, source.len())?
        .map(|region| WindowSource::new(source, region.start, region.end - region.start).ok_or(Error::BadOffset))
        .transpose()
}

#[derive(Debug, Clone, Copy)]
pub struct LandmarkDirectory {
    pub count: u32,
    pub payload: u32,
    pub len: u32,
}

impl LandmarkDirectory {
    /// `source` is the header-declared section window, not the whole map.
    pub fn read(source: &dyn ByteSource) -> Result<Self, Error> {
        let mut bytes = [0; SECTION_HEADER_LEN];
        source.read_at(0, &mut bytes).map_err(Error::Source)?;
        let directory = Self { count: rd_u32(&bytes, 0), payload: rd_u32(&bytes, 8), len: rd_u32(&bytes, 12) };
        if rd_u16(&bytes, 6) != SECTION_VERSION {
            return Err(Error::BadVersion);
        }
        if directory.count > MAX_RECORDS
            || rd_u16(&bytes, 4) as usize != RECORD_LEN
            || directory.payload != SECTION_HEADER_LEN as u32 + directory.count * RECORD_LEN as u32
            || directory.len < directory.payload
            || u64::from(directory.len) > source.len()
        {
            return Err(Error::BadOffset);
        }
        Ok(directory)
    }

    pub fn record(&self, source: &dyn ByteSource, index: u32) -> Result<LandmarkRecord, Error> {
        if index >= self.count {
            return Err(Error::BadOffset);
        }
        let mut bytes = [0; RECORD_LEN];
        source
            .read_at(SECTION_HEADER_LEN as u64 + u64::from(index) * RECORD_LEN as u64, &mut bytes)
            .map_err(Error::Source)?;
        LandmarkRecord::decode(&bytes).ok_or(Error::BadOffset)
    }

    pub fn content<'a>(
        &self,
        source: &'a dyn ByteSource,
        reference: ContentRef,
        limit: u32,
    ) -> Result<WindowSource<'a>, Error> {
        let range = reference.range(self.payload, self.len, limit).ok_or(Error::BadOffset)?;
        WindowSource::new(source, range.start, range.end - range.start).ok_or(Error::BadOffset)
    }

    pub fn article(
        &self,
        source: &dyn ByteSource,
        record: &LandmarkRecord,
        preferred: [u8; 2],
    ) -> Result<obc_formats::articles::ArticleVariant, Error> {
        let bundle = self.content(source, record.articles, obc_formats::articles::MAX_BYTES)?;
        let mut article = crate::articles::select(&bundle, preferred)?;
        article.text.offset = article.text.offset.checked_add(record.articles.offset).ok_or(Error::BadOffset)?;
        article.attribution.offset =
            article.attribution.offset.checked_add(record.articles.offset).ok_or(Error::BadOffset)?;
        Ok(article)
    }

    pub fn name<'a>(
        &self,
        source: &dyn ByteSource,
        record: &LandmarkRecord,
        output: &'a mut [u8; MAX_NAME_BYTES as usize],
    ) -> Result<&'a str, Error> {
        let name = self.content(source, record.name, MAX_NAME_BYTES)?;
        let bytes = &mut output[..name.len() as usize];
        name.read_at(0, bytes).map_err(Error::Source)?;
        core::str::from_utf8(bytes).map_err(|_| Error::BadOffset)
    }
}

/// Length-prefixed UTF-8 fields: count u16, count+1 byte offsets u32, then data.
pub fn page<'a>(
    source: &dyn ByteSource,
    expected_count: u16,
    index: u16,
    output: &'a mut [u8],
) -> Result<&'a str, Error> {
    let mut count = [0; 2];
    source.read_at(0, &mut count).map_err(Error::Source)?;
    if rd_u16(&count, 0) != expected_count || expected_count == 0 || index >= expected_count {
        return Err(Error::BadOffset);
    }
    let payload = 2 + (u64::from(expected_count) + 1) * 4;
    let mut bounds = [0; 8];
    source.read_at(2 + u64::from(index) * 4, &mut bounds).map_err(Error::Source)?;
    let start = u64::from(rd_u32(&bounds, 0));
    let end = u64::from(rd_u32(&bounds, 4));
    if start < payload || end < start || end > source.len() || end - start > output.len() as u64 {
        return Err(Error::BadOffset);
    }
    let bytes = &mut output[..(end - start) as usize];
    source.read_at(start, bytes).map_err(Error::Source)?;
    core::str::from_utf8(bytes).map_err(|_| Error::BadOffset)
}

pub const PAGE_SIZE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct LandmarkKey {
    pub distance_m: u32,
    pub qid: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct LandmarkHit {
    pub index: u32,
    pub position: (i32, i32),
    pub key: LandmarkKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryProgress {
    Pending,
    Ready { more: bool },
    Failed(Error),
    Cancelled,
}

/// One frozen position, scope and source generation. A page uses one record read
/// per step, first bisecting the latitude index, then scanning only its latitude band.
pub struct LandmarkQuery {
    generation: u32,
    center: (i32, i32),
    radius_m: u32,
    latitude: (i32, i32),
    cosine: f32,
    lower: u32,
    upper: u32,
    cursor: Option<u32>,
    after: Option<LandmarkKey>,
    more: bool,
    progress: QueryProgress,
}

impl LandmarkQuery {
    pub fn new(
        directory: LandmarkDirectory,
        generation: u32,
        center: (i32, i32),
        radius_m: u32,
        after: Option<LandmarkKey>,
    ) -> Self {
        // 110 km per degree conservatively includes the latitude span at every latitude.
        let span = (u64::from(radius_m) * 1_000_000 / 110_000 + 1).min(180_000_000) as i32;
        Self {
            generation,
            center,
            radius_m,
            latitude: (center.1.saturating_sub(span), center.1.saturating_add(span)),
            cosine: cos_lat(center.1),
            lower: 0,
            upper: directory.count,
            cursor: None,
            after,
            more: false,
            progress: QueryProgress::Pending,
        }
    }

    pub fn cancel(&mut self) {
        self.progress = QueryProgress::Cancelled;
    }

    pub fn step<const N: usize>(
        &mut self,
        source: &dyn ByteSource,
        directory: LandmarkDirectory,
        generation: u32,
        output: &mut Vec<LandmarkHit, N>,
    ) -> QueryProgress {
        if generation != self.generation {
            self.cancel();
        }
        if self.progress == QueryProgress::Cancelled {
            output.clear();
        }
        if self.progress != QueryProgress::Pending {
            return self.progress;
        }
        if let Err(error) = self.advance(source, directory, output) {
            self.progress = QueryProgress::Failed(error);
            output.clear();
        }
        self.progress
    }

    fn advance<const N: usize>(
        &mut self,
        source: &dyn ByteSource,
        directory: LandmarkDirectory,
        output: &mut Vec<LandmarkHit, N>,
    ) -> Result<(), Error> {
        let Some(cursor) = self.cursor else {
            if self.lower < self.upper {
                let middle = self.lower + (self.upper - self.lower) / 2;
                if directory.record(source, middle)?.lat < self.latitude.0 {
                    self.lower = middle + 1;
                } else {
                    self.upper = middle;
                }
            } else {
                self.cursor = Some(self.lower);
            }
            return Ok(());
        };
        if cursor == directory.count {
            self.progress = QueryProgress::Ready { more: self.more };
            return Ok(());
        }
        let record = directory.record(source, cursor)?;
        self.cursor = Some(cursor + 1);
        if record.lat > self.latitude.1 {
            self.progress = QueryProgress::Ready { more: self.more };
            return Ok(());
        }
        let distance = ground_dist_m_cl(self.center, (record.lon, record.lat), self.cosine);
        let key = LandmarkKey { distance_m: (distance + 0.5) as u32, qid: record.qid };
        if distance > self.radius_m as f32 || self.after.is_some_and(|after| key <= after) {
            return Ok(());
        }
        if output.iter().any(|hit| hit.key.qid == key.qid) {
            return Ok(());
        }
        let at = output.partition_point(|hit| hit.key < key);
        if output.is_full() {
            self.more = true;
            if at == N {
                return Ok(());
            }
            output.pop();
        }
        output
            .insert(at, LandmarkHit { index: cursor, position: (record.lon, record.lat), key })
            .map_err(|_| Error::BadOffset)?;
        Ok(())
    }
}
