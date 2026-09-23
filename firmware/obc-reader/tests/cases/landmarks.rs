use heapless::Vec;
use obc_formats::{
    io::{ByteSource, Error as SourceError, SliceSource},
    obcm::{landmarks::*, POI_HOURS_REF_NONE},
};
use obc_reader::{landmarks::*, Error};
use std::cell::Cell;

fn section(count: u32) -> std::vec::Vec<u8> {
    let payload = SECTION_HEADER_LEN as u32 + count * RECORD_LEN as u32;
    let mut bytes = std::vec![0; payload as usize];
    bytes[..4].copy_from_slice(&count.to_le_bytes());
    bytes[4..6].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
    bytes[6..8].copy_from_slice(&SECTION_VERSION.to_le_bytes());
    bytes[8..12].copy_from_slice(&payload.to_le_bytes());
    bytes[12..16].copy_from_slice(&(payload + 4).to_le_bytes());
    for i in 0..count {
        let record = LandmarkRecord {
            qid: u64::from(i) + 1,
            lon: 8_000_000,
            lat: 46_000_000,
            category: 2,
            hours_ref: POI_HOURS_REF_NONE,
            osm: None,
            name: ContentRef { offset: payload, len: 4 },
            articles: ContentRef::default(),
            photo: ContentRef::default(),
            photo_attribution: ContentRef::default(),
        };
        let start = SECTION_HEADER_LEN + i as usize * RECORD_LEN;
        bytes[start..start + RECORD_LEN].copy_from_slice(&record.encode());
    }
    bytes.extend_from_slice(b"Site");
    bytes
}

struct Source<'a> {
    bytes: &'a [u8],
    reads: Cell<usize>,
    fail: Cell<bool>,
}
impl ByteSource for Source<'_> {
    fn len(&self) -> u64 {
        self.bytes.len() as u64
    }
    fn read_at(&self, offset: u64, output: &mut [u8]) -> Result<(), SourceError> {
        self.reads.set(self.reads.get() + 1);
        if self.fail.get() {
            return Err(SourceError::Io);
        }
        SliceSource(self.bytes).read_at(offset, output)
    }
}

#[test]
fn pages_reach_every_colocated_identity_with_one_read_per_step() {
    let bytes = section(19);
    let source = Source { bytes: &bytes, reads: Cell::new(0), fail: Cell::new(false) };
    let directory = LandmarkDirectory::read(&source).unwrap();
    let mut after = None;
    let mut seen = std::vec::Vec::new();
    loop {
        let mut query = LandmarkQuery::new(directory, 42, (8_000_000, 46_000_000), 10_000, after);
        let mut output = Vec::<LandmarkHit, PAGE_SIZE>::new();
        let more = loop {
            let before = source.reads.get();
            let progress = query.step(&source, directory, 42, &mut output);
            assert!(source.reads.get() - before <= 1);
            match progress {
                QueryProgress::Pending => (),
                QueryProgress::Ready { more } => break more,
                other => panic!("unexpected query result: {other:?}"),
            }
        };
        let mut name = [0; MAX_NAME_BYTES as usize];
        for hit in &output {
            assert_eq!(
                directory.name(&source, &directory.record(&source, hit.index).unwrap(), &mut name).unwrap(),
                "Site"
            );
            seen.push(hit.key.qid);
        }
        if !more {
            break;
        }
        after = Some(output.last().unwrap().key);
    }
    assert_eq!(seen, (1..=19).collect::<std::vec::Vec<_>>());
}

#[test]
fn source_failure_and_generation_change_clear_partial_results() {
    let bytes = section(3);
    let source = Source { bytes: &bytes, reads: Cell::new(0), fail: Cell::new(false) };
    let directory = LandmarkDirectory::read(&source).unwrap();
    for cancel in [false, true] {
        source.fail.set(false);
        let mut query = LandmarkQuery::new(directory, 42, (8_000_000, 46_000_000), 10_000, None);
        let mut output = Vec::<LandmarkHit, PAGE_SIZE>::new();
        while output.is_empty() {
            assert_eq!(query.step(&source, directory, 42, &mut output), QueryProgress::Pending);
        }
        source.fail.set(!cancel);
        let expected =
            if cancel { QueryProgress::Cancelled } else { QueryProgress::Failed(Error::Source(SourceError::Io)) };
        assert_eq!(query.step(&source, directory, if cancel { 43 } else { 42 }, &mut output), expected);
        assert!(output.is_empty());
        source.fail.set(false);
        assert_eq!(query.step(&source, directory, 42, &mut output), expected);
    }
}

#[test]
fn malformed_record_fails_query_but_bad_photo_reference_is_selected_item_error() {
    let mut bytes = section(1);
    let photo = SECTION_HEADER_LEN + 68;
    bytes[photo..photo + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    bytes[photo + 4..photo + 8].copy_from_slice(&100u32.to_le_bytes());
    let source = SliceSource(&bytes);
    let directory = LandmarkDirectory::read(&source).unwrap();
    let record = directory.record(&source, 0).unwrap();
    assert!(matches!(directory.content(&source, record.photo, PHOTO_MAX_COMPRESSED as u32), Err(Error::BadOffset)));
    assert_eq!(directory.name(&source, &record, &mut [0; MAX_NAME_BYTES as usize]).unwrap(), "Site");
    bytes[SECTION_HEADER_LEN + 16] = 255;
    let source = SliceSource(&bytes);
    let mut query = LandmarkQuery::new(directory, 42, (8_000_000, 46_000_000), 10_000, None);
    assert_eq!(
        query.step(&source, directory, 42, &mut Vec::<LandmarkHit, PAGE_SIZE>::new()),
        QueryProgress::Failed(Error::BadOffset)
    );
}

#[test]
fn content_references_and_utf8_page_boundaries_are_checked() {
    let bytes = [2, 0, 14, 0, 0, 0, 16, 0, 0, 0, 19, 0, 0, 0, b'A', b'.', 0xc3, 0xa9, b'.'];
    let mut output = [0; MAX_PAGE_BYTES];
    assert_eq!(page(&SliceSource(&bytes), 2, 1, &mut output).unwrap(), "é.");
    assert!(page(&SliceSource(&bytes), 2, 2, &mut output).is_err());
    let mut invalid = bytes;
    invalid[6] = 17;
    assert!(page(&SliceSource(&invalid), 2, 1, &mut output).is_err());
    invalid[6] = 0;
    assert!(page(&SliceSource(&invalid), 2, 1, &mut output).is_err());
    assert!(ContentRef { offset: u32::MAX, len: 10 }.range(16, u32::MAX, 10).is_none());
    assert!(ContentRef { offset: 0, len: 10 }.range(16, 100, 10).is_none());
}

#[test]
fn multilingual_bundle_selects_ui_english_and_local_default_and_checks_directory() {
    use obcm_testkit::articles::bundle;
    let credits = ["en.wikipedia.org/?oldid=1", "Title", "Wikipedia contributors", "CC BY-SA 4.0"];
    let variants = [
        (*b"de", &["Deutsch."][..], &credits[..]),
        (*b"en", &["English."][..], &credits[..]),
        (*b"fr", &["Français."][..], &credits[..]),
    ];
    let bytes = bundle(*b"fr", &variants);
    for (requested, expected) in [(*b"de", "Deutsch."), (*b"es", "English.")] {
        let source = SliceSource(&bytes);
        let article = obc_reader::articles::select(&source, requested).unwrap();
        let text =
            obc_formats::io::WindowSource::new(&source, article.text.offset as u64, article.text.len as u64).unwrap();
        assert_eq!(page(&text, article.text_pages as u16, 0, &mut [0; MAX_PAGE_BYTES]).unwrap(), expected);
    }
    let bytes = bundle(*b"fr", &[variants[0], variants[2]]);
    assert_eq!(obc_reader::articles::select(&SliceSource(&bytes), *b"es").unwrap().language, *b"fr");
    let duplicate = bundle(*b"fr", &[variants[2], variants[2]]);
    assert!(obc_reader::articles::select(&SliceSource(&duplicate), *b"fr").is_err());
    let absent = bundle(*b"es", &[variants[2]]);
    assert!(obc_reader::articles::select(&SliceSource(&absent), *b"fr").is_err());
    let mut invalid = bytes;
    invalid[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(obc_reader::articles::select(&SliceSource(&invalid), *b"fr").is_err());
}
