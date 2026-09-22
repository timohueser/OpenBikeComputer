use obc_formats::{
    io::{ByteSource, SliceSource},
    obcm::{self, landmarks::MAX_PAGE_BYTES, peaks::*, SourceId},
};
use obc_reader::{peaks::*, MapCache, MapTables, Reader};

fn vector() -> Vec<u8> {
    include_bytes!("../../../../specs/vectors/peak-section-v17.bin").to_vec()
}
fn map(section: &[u8]) -> Vec<u8> {
    let mut map = obcm_testkit::build_poi_map((7_000_000, 46_000_000, 9_000_000, 48_000_000), 512, &[]);
    let start = obcm_testkit::align_up(map.len());
    map.resize(start, 0xff);
    map.extend(section);
    map.resize(obcm_testkit::align_up(map.len()), 0xff);
    let len = map.len() - start;
    map[obcm::HEADER_PEAK_OFFSET_OFF..obcm::HEADER_PEAK_OFFSET_OFF + 4]
        .copy_from_slice(&obcm_testkit::scaled(start).to_le_bytes());
    map[obcm::HEADER_PEAK_LENGTH_OFF..obcm::HEADER_PEAK_LENGTH_OFF + 4]
        .copy_from_slice(&obcm_testkit::scaled(len).to_le_bytes());
    map
}
fn text(reader: &Reader<'_>, selection: Selection, preferred: [u8; 2]) -> Result<String, obc_reader::Error> {
    reader.with_peak_article(selection, |section, d, r| {
        let bundle = d.content(section, &r, 1, obc_formats::articles::MAX_BYTES)?;
        let variant = obc_reader::articles::select(&bundle, preferred)?;
        let pages =
            obc_formats::io::WindowSource::new(&bundle, variant.text.offset.into(), variant.text.len.into()).unwrap();
        Ok(obc_reader::landmarks::page(&pages, variant.text_pages.into(), 0, &mut [0; MAX_PAGE_BYTES])?.to_owned())
    })
}
#[test]
fn direct_multilingual_lookup_is_separate_and_generation_bound() {
    let bytes = map(&vector());
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let r = Reader::new(&src, &tables, &cache);
    assert!(obc_reader::landmarks::map_section(&src).unwrap().is_none());
    let first = r.peak_article(SourceId::osm(1, 101)).unwrap().unwrap();
    let shared = r.peak_article(SourceId::osm(1, 102)).unwrap().unwrap();
    assert_eq!(text(&r, first, *b"de").unwrap(), "Ein gemeinsamer Berg.");
    assert_eq!(text(&r, shared, *b"es").unwrap(), "A shared mountain.");
    let local = r.peak_article(SourceId::osm(1, 103)).unwrap().unwrap();
    assert_eq!(text(&r, local, *b"de").unwrap(), "Une montagne.");
    assert!(r.peak_article(SourceId::osm(1, 104)).unwrap().is_none());
    assert!(r.peak_article(SourceId::osm(2, 101)).unwrap().is_none());
    let new_tables = MapTables::parse(&src).unwrap();
    let changed = Reader::new(&src, &new_tables, &cache);
    assert!(text(&changed, first, *b"en").is_err());
}
/// Content is text or a photo: Peak View marks a summit for either, and for neither it marks none.
#[test]
fn a_record_with_a_photo_and_no_text_is_content_and_an_empty_record_is_not() {
    let original = vector();
    let records = HEADER_LEN + 3 * ASSOCIATION_LEN;
    let mut section = original.clone();
    // Move the first record's bundle, guard byte and all, into its photo slot, leaving no text.
    let reference = original[records + 40..records + 48].to_vec();
    let payload = u32::from_le_bytes(reference[..4].try_into().unwrap()) as usize;
    section[payload + 32] = 2;
    section[records + 40..records + 48].fill(0);
    section[records + 48..records + 56].copy_from_slice(&reference);
    let bytes = map(&section);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let cache = MapCache::new();
    let reader = Reader::new(&src, &tables, &cache);
    assert!(reader.peak_article(SourceId::osm(1, 101)).unwrap().is_some());
    assert!(text(&reader, reader.peak_article(SourceId::osm(1, 101)).unwrap().unwrap(), *b"en").is_err());
    section[records + 48..records + 56].fill(0);
    let bytes = map(&section);
    let src = SliceSource(&bytes);
    let tables = MapTables::parse(&src).unwrap();
    let reader = Reader::new(&src, &tables, &cache);
    assert!(reader.peak_article(SourceId::osm(1, 101)).is_err());
}
#[test]
fn corrupt_index_and_in_bounds_payload_swaps_fail_closed() {
    let original = vector();
    let records = HEADER_LEN + 3 * ASSOCIATION_LEN;
    let cases = [
        (HEADER_LEN + 40, 1u32.to_le_bytes().to_vec()), // another valid article index
        (HEADER_LEN + 40, u32::MAX.to_le_bytes().to_vec()),
        (records + 40, original[records + RECORD_LEN + 40..records + RECORD_LEN + 48].to_vec()), // another article's valid bundle
        (records + 40, original[records + 32..records + 40].to_vec()), // same article, wrong payload kind
        (records + 40, vec![0; 8]),
        (records + 44, u32::MAX.to_le_bytes().to_vec()),
    ];
    for (at, change) in cases {
        let mut section = original.clone();
        section[at..at + change.len()].copy_from_slice(&change);
        let bytes = map(&section);
        let src = SliceSource(&bytes);
        let t = MapTables::parse(&src).unwrap();
        let c = MapCache::new();
        let r = Reader::new(&src, &t, &c);
        assert!(r.peak_article(SourceId::osm(1, 101)).is_err(), "corruption at {at}");
    }
    for end in [0, HEADER_LEN - 1, records, original.len() - 1] {
        let bytes = vector();
        let source = SliceSource(&bytes[..end]);
        assert!(Directory::read(&source).is_err());
    }
}
#[test]
fn lookup_has_logarithmic_reads_and_small_stack_buffers() {
    struct Bounded {
        bytes: Vec<u8>,
        reads: std::cell::Cell<usize>,
    }
    impl ByteSource for Bounded {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&self, offset: u64, out: &mut [u8]) -> Result<(), obc_formats::io::Error> {
            assert!(out.len() <= 64);
            self.reads.set(self.reads.get() + 1);
            SliceSource(&self.bytes).read_at(offset, out)
        }
    }
    let count = MAX_ASSOCIATIONS;
    let payload = HEADER_LEN as u32 + count * ASSOCIATION_LEN as u32 + RECORD_LEN as u32;
    let mut b = vec![0; payload as usize];
    b[..2].copy_from_slice(&VERSION.to_le_bytes());
    b[2..4].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
    b[4..8].copy_from_slice(&count.to_le_bytes());
    b[8] = 1;
    b[12..16].copy_from_slice(&payload.to_le_bytes());
    b[16..20].copy_from_slice(&payload.to_le_bytes());
    for i in 0..count {
        let at = HEADER_LEN + i as usize * ASSOCIATION_LEN;
        b[at..at + ASSOCIATION_LEN].copy_from_slice(
            &Association { source: SourceId::osm(1, u64::from(i) + 1), article: [1; 32], index: 0 }.encode(),
        );
    }
    b[payload as usize - RECORD_LEN..payload as usize - RECORD_LEN + 32].fill(1);
    let source = Bounded { bytes: b, reads: Default::default() };
    let d = Directory::read(&source).unwrap();
    assert!(d.find(&source, SourceId::osm(1, u64::from(count))).unwrap().is_some());
    assert!(source.reads.get() <= 22);
}
