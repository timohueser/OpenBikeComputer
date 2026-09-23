use obc_formats::obcm::{landmarks::*, HEADER_LANDMARK_LENGTH_OFF, HEADER_LANDMARK_OFFSET_OFF, POI_HOURS_REF_NONE};

/// Authored metadata and payloads for assembly contracts; no captured article or image content.
pub fn record(qid: u64) -> LandmarkRecord {
    LandmarkRecord {
        qid,
        lon: 7_500_000,
        lat: 47_300_000,
        category: 1,
        hours_ref: POI_HOURS_REF_NONE,
        osm: None,
        name: ContentRef::default(),
        articles: ContentRef::default(),
        photo: ContentRef::default(),
        photo_attribution: ContentRef::default(),
    }
}

const TEXT: &str = "An alpine château.";
const ARTICLE: [&str; 4] = ["en.wikipedia.org/?oldid=17", "Castle", "Wikipedia contributors", "CC BY-SA 4.0"];
const PHOTO: [&str; 4] = ["Wikimedia Commons", "Castle.jpg", "Authored photographer", "CC BY 4.0"];

fn fields(values: &[&str]) -> Vec<u8> {
    let mut bytes = (values.len() as u16).to_le_bytes().to_vec();
    let mut offset = 2 + (values.len() as u32 + 1) * 4;
    for value in values {
        bytes.extend_from_slice(&offset.to_le_bytes());
        offset += value.len() as u32;
    }
    bytes.extend_from_slice(&offset.to_le_bytes());
    for value in values {
        bytes.extend_from_slice(value.as_bytes());
    }
    bytes
}

fn pixel(index: usize) -> u8 {
    ((index * 31 + index / 23) % 64) as u8
}

/// Independent zlib authoring: a 4 KiB window and one final uncompressed DEFLATE block.
fn photo() -> Vec<u8> {
    let cmf = ((PHOTO_WINDOW_BITS - 8) << 4) | 8;
    let flags = ((31 - ((cmf as u16) << 8) % 31) % 31) as u8;
    let mut bytes = vec![cmf, flags, 1];
    let len = PHOTO_PIXELS as u16;
    bytes.extend_from_slice(&len.to_le_bytes());
    bytes.extend_from_slice(&(!len).to_le_bytes());
    let (mut a, mut b) = (1u32, 0u32);
    for index in 0..PHOTO_PIXELS {
        let value = pixel(index);
        bytes.push(value);
        a = (a + value as u32) % 65521;
        b = (b + a) % 65521;
    }
    bytes.extend_from_slice(&((b << 16) | a).to_be_bytes());
    assert!(bytes.len() <= PHOTO_MAX_COMPRESSED);
    bytes
}

pub fn assert_content(source: &dyn obc_formats::io::ByteSource) {
    use obc_reader::{
        landmarks::{page, LandmarkDirectory},
        photo::{PhotoDecoder, Progress},
    };
    let directory = LandmarkDirectory::read(source).unwrap();
    for index in 0..directory.count {
        let record = directory.record(source, index).unwrap();
        assert_eq!(directory.name(source, &record, &mut [0; MAX_NAME_BYTES as usize]).unwrap(), "Castle");
        let french = directory.article(source, &record, *b"fr").unwrap();
        let text = directory.content(source, french.text, MAX_TEXT_BYTES).unwrap();
        assert_eq!(page(&text, french.text_pages as u16, 0, &mut [0; MAX_PAGE_BYTES]).unwrap(), "Un château.");
        let article = directory.article(source, &record, *b"en").unwrap();
        let text = directory.content(source, article.text, MAX_TEXT_BYTES).unwrap();
        assert_eq!(page(&text, article.text_pages as u16, 0, &mut [0; MAX_PAGE_BYTES]).unwrap(), TEXT);
        for (reference, expected) in [(article.attribution, ARTICLE), (record.photo_attribution, PHOTO)] {
            let credit = directory.content(source, reference, MAX_ATTRIBUTION_BYTES).unwrap();
            for (index, expected) in expected.iter().enumerate() {
                assert_eq!(page(&credit, CREDIT_FIELDS, index as u16, &mut [0; MAX_PAGE_BYTES]).unwrap(), *expected);
            }
        }
        let photo = directory.content(source, record.photo, PHOTO_MAX_COMPRESSED as u32).unwrap();
        let mut decoder = PhotoDecoder::new();
        let mut completed = false;
        let mut written = 0;
        for _ in 0..1024 {
            let progress = decoder
                .step(&photo, |offset, values| {
                    assert_eq!(offset, written);
                    assert!(values.len() <= PHOTO_HISTORY);
                    for (i, value) in values.iter().enumerate() {
                        assert_eq!(*value, pixel(offset + i));
                    }
                    written += values.len();
                })
                .unwrap();
            if progress == Progress::Complete {
                completed = true;
                break;
            }
        }
        assert!(completed);
        assert_eq!(written, PHOTO_PIXELS);
    }
}

pub fn attach(map: &mut Vec<u8>, mut records: Vec<LandmarkRecord>, photo: bool) {
    records.sort_by_key(LandmarkRecord::key);
    let payload = SECTION_HEADER_LEN + records.len() * RECORD_LEN;
    let mut section = vec![0; payload];
    section[..4].copy_from_slice(&(records.len() as u32).to_le_bytes());
    section[4..6].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
    section[6..8].copy_from_slice(&SECTION_VERSION.to_le_bytes());
    section[8..12].copy_from_slice(&(payload as u32).to_le_bytes());
    let mut add = |bytes: &[u8]| {
        let reference = ContentRef { offset: section.len() as u32, len: bytes.len() as u32 };
        section.extend_from_slice(bytes);
        reference
    };
    for record in &mut records {
        record.name = add(b"Castle");
        record.articles = add(&obcm_testkit::articles::bundle(
            *b"en",
            &[(*b"en", &[TEXT], &ARTICLE), (*b"fr", &["Un château."], &ARTICLE)],
        ));
        if photo {
            record.photo = add(&self::photo());
            record.photo_attribution = add(&fields(&PHOTO));
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
