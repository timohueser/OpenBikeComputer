//! Landmark content limits shared by the map producer and reader.

use super::{PoiMetadata, POI_HOURS_REF_NONE};
use crate::io::{rd_i32, rd_u16, rd_u32};

pub const SECTION_HEADER_LEN: usize = 16;
pub const RECORD_LEN: usize = 84;
pub const SECTION_VERSION: u16 = 1;
pub const MAX_RECORDS: u32 = 65_535;
pub const MAX_NAME_BYTES: u32 = 256;
pub const MAX_PAGE_BYTES: usize = 1024;
pub const MAX_TEXT_PAGES: u8 = 4;
pub const MAX_TEXT_BYTES: u32 = 2 + (MAX_TEXT_PAGES as u32 + 1) * 4 + MAX_TEXT_PAGES as u32 * MAX_PAGE_BYTES as u32;
pub const MAX_CREDIT_PAGES: u16 = 256;
pub const MAX_ATTRIBUTION_BYTES: u32 = 65_535;

/// Byte offsets are relative to the landmark section, independent of OBCM scale.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ContentRef {
    pub offset: u32,
    pub len: u32,
}

impl ContentRef {
    pub fn range(self, payload: u32, section_len: u32, limit: u32) -> Option<core::ops::Range<u64>> {
        let end = self.offset.checked_add(self.len)?;
        (self.len > 0 && self.len <= limit && self.offset >= payload && end <= section_len)
            .then_some(u64::from(self.offset)..u64::from(end))
    }

    pub fn is_absent(self) -> bool {
        self.offset == 0 && self.len == 0
    }

    fn decode(bytes: &[u8], at: usize) -> Self {
        Self { offset: rd_u32(bytes, at), len: rd_u32(bytes, at + 4) }
    }

    fn encode(self, bytes: &mut [u8], at: usize) {
        bytes[at..at + 4].copy_from_slice(&self.offset.to_le_bytes());
        bytes[at + 4..at + 8].copy_from_slice(&self.len.to_le_bytes());
    }
}

/// Records form a latitude index, ordered by `(lat, lon, qid)`.
/// Content is fetched only after selection; a bad photo does not erase its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LandmarkRecord {
    pub qid: u64,
    pub lon: i32,
    pub lat: i32,
    pub category: u8,
    pub hours_ref: u16,
    pub osm: Option<PoiMetadata>,
    pub name: ContentRef,
    pub articles: ContentRef,
    pub photo: ContentRef,
    pub photo_attribution: ContentRef,
}

impl LandmarkRecord {
    pub fn key(&self) -> (i32, i32, u64) {
        (self.lat, self.lon, self.qid)
    }

    pub fn decode(bytes: &[u8; RECORD_LEN]) -> Option<Self> {
        let osm = if bytes[24..52].iter().all(|&b| b == 0) { None } else { Some(PoiMetadata::decode(&bytes[24..52])?) };
        let record = Self {
            qid: u64::from_le_bytes(bytes[..8].try_into().ok()?),
            lon: rd_i32(bytes, 8),
            lat: rd_i32(bytes, 12),
            category: bytes[16],
            hours_ref: rd_u16(bytes, 20),
            osm,
            name: ContentRef::decode(bytes, 52),
            articles: ContentRef::decode(bytes, 60),
            photo: ContentRef::decode(bytes, 68),
            photo_attribution: ContentRef::decode(bytes, 76),
        };
        (record.qid > 0
            && (-90_000_000..=90_000_000).contains(&record.lat)
            && (-180_000_000..=180_000_000).contains(&record.lon)
            && (1..=16).contains(&record.category)
            && bytes[17..20] == [0; 3]
            && bytes[22..24] == [0, 0]
            && (record.osm.is_some() || record.hours_ref == POI_HOURS_REF_NONE))
            .then_some(record)
    }

    pub fn encode(&self) -> [u8; RECORD_LEN] {
        let mut bytes = [0; RECORD_LEN];
        bytes[..8].copy_from_slice(&self.qid.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.lon.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.lat.to_le_bytes());
        bytes[16] = self.category;
        bytes[20..22].copy_from_slice(&self.hours_ref.to_le_bytes());
        if let Some(osm) = self.osm {
            bytes[24..52].copy_from_slice(&osm.encode());
        }
        for (reference, at) in [(self.name, 52), (self.articles, 60), (self.photo, 68), (self.photo_attribution, 76)] {
            reference.encode(&mut bytes, at);
        }
        bytes
    }
}

pub const PHOTO_WIDTH: usize = 216;
pub const PHOTO_HEIGHT: usize = 240;
pub const PHOTO_PIXELS: usize = PHOTO_WIDTH * PHOTO_HEIGHT;
/// Each photo is one zlib stream with at most a 4 KiB DEFLATE history window.
pub const PHOTO_WINDOW_BITS: u8 = 12;
pub const PHOTO_HISTORY: usize = 1 << PHOTO_WINDOW_BITS;
/// Includes the zlib wrapper and worst-case stored-block overhead.
pub const PHOTO_MAX_COMPRESSED: usize = PHOTO_PIXELS + 256;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landmark_category_accepts_the_table_and_rejects_other_wire_ids() {
        let record = LandmarkRecord {
            qid: 1,
            lon: 0,
            lat: 0,
            category: 1,
            hours_ref: POI_HOURS_REF_NONE,
            osm: None,
            name: ContentRef::default(),
            articles: ContentRef::default(),
            photo: ContentRef::default(),
            photo_attribution: ContentRef::default(),
        };

        for category in 1..=16 {
            let bytes = LandmarkRecord { category, ..record }.encode();
            assert_eq!(LandmarkRecord::decode(&bytes).map(|decoded| decoded.category), Some(category));
        }
        for category in [0, 17, u8::MAX] {
            let bytes = LandmarkRecord { category, ..record }.encode();
            assert!(LandmarkRecord::decode(&bytes).is_none());
        }
    }
}
