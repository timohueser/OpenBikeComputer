use obc_formats::obcm::{landmarks::*, HEADER_LANDMARK_LENGTH_OFF, HEADER_LANDMARK_OFFSET_OFF, POI_HOURS_REF_NONE};

/// Authored metadata and payloads for assembly contracts; no captured article or image content.
pub fn record(qid: u64) -> LandmarkRecord {
    LandmarkRecord {
        qid,
        lon: 7_500_000,
        lat: 47_300_000,
        category: 1,
        language: *b"en",
        text_pages: 1,
        hours_ref: POI_HOURS_REF_NONE,
        osm: None,
        name: ContentRef::default(),
        text: ContentRef::default(),
        article: ContentRef::default(),
        photo: ContentRef::default(),
        photo_attribution: ContentRef::default(),
    }
}

pub fn attach(map: &mut Vec<u8>, mut records: Vec<LandmarkRecord>, photo: bool) {
    records.sort_by_key(LandmarkRecord::key);
    let payload = SECTION_HEADER_LEN + records.len() * RECORD_LEN;
    let mut section = vec![0; payload];
    section[..4].copy_from_slice(&(records.len() as u32).to_le_bytes());
    section[4..6].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
    section[8..12].copy_from_slice(&(payload as u32).to_le_bytes());
    let mut add = |bytes: &[u8]| {
        let reference = ContentRef { offset: section.len() as u32, len: bytes.len() as u32 };
        section.extend_from_slice(bytes);
        reference
    };
    for record in &mut records {
        record.name = add(b"Castle");
        record.text = add(b"\x01\x00\x0a\x00\x00\x00\x0b\x00\x00\x00X");
        record.article = add(b"authored credit");
        if photo {
            record.photo = add(&vec![0xA5; 12_000]);
            record.photo_attribution = add(b"authored photo credit");
        }
    }
    for (index, record) in records.iter().enumerate() {
        let start = SECTION_HEADER_LEN + index * RECORD_LEN;
        section[start..start + RECORD_LEN].copy_from_slice(&record.encode());
    }
    let exact_len = section.len() as u32;
    section[12..16].copy_from_slice(&exact_len.to_le_bytes());
    map.resize(map.len().next_multiple_of(16), 0xFF);
    let start = map.len();
    map.extend_from_slice(&section);
    map.resize(map.len().next_multiple_of(16), 0xFF);
    map[HEADER_LANDMARK_OFFSET_OFF..HEADER_LANDMARK_OFFSET_OFF + 4]
        .copy_from_slice(&((start / 16) as u32).to_le_bytes());
    let length = ((map.len() - start) / 16) as u32;
    map[HEADER_LANDMARK_LENGTH_OFF..HEADER_LANDMARK_LENGTH_OFF + 4].copy_from_slice(&length.to_le_bytes());
}
