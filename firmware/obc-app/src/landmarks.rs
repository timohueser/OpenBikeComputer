//! Frozen nearby identities and one prepared source page. All map reads happen before draw.
use crate::{
    photo::Selection,
    screen::{Screen, Transition},
};
use obc_formats::{
    io::{rd_u16, ByteSource},
    obcm::landmarks::*,
};
use obc_reader::{
    landmarks::{map_section, page, LandmarkDirectory, LandmarkHit, LandmarkKey, LandmarkQuery, QueryProgress},
    Error, Reader,
};

const ROWS: usize = 4;
const RADIUS_M: u32 = 10_000;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Idle,
    Loading,
    Ready,
    Empty,
    Missing,
    Unsupported,
    Failed,
    Partial,
    NoFix,
    NoMap,
    Stale,
}
pub type Row = LandmarkHit;
/// One content buffer serves both reading and attribution. Row identities do not change while reading.
pub struct Landmarks {
    pub status: Status,
    pub rows: heapless::Vec<Row, ROWS>,
    pub origin: (i32, i32),
    pub selected: usize,
    pub reading: bool,
    pub page: u16,
    pub source_page: u16,
    pub source_pages: u16,
    pub name: heapless::String<256>,
    pub text: heapless::String<MAX_PAGE_BYTES>,
    pub record: Option<LandmarkRecord>,
    pub generation: Option<u32>,
    pub more: bool,
    query: Option<LandmarkQuery>,
    after: Option<LandmarkKey>,
    loaded: Option<(usize, bool, u16)>,
    article_pages: u16,
}
impl Landmarks {
    pub const fn new() -> Self {
        Self {
            status: Status::Idle,
            rows: heapless::Vec::new(),
            origin: (0, 0),
            selected: 0,
            reading: false,
            page: 0,
            source_page: 0,
            source_pages: 0,
            name: heapless::String::new(),
            text: heapless::String::new(),
            record: None,
            generation: None,
            more: false,
            query: None,
            after: None,
            loaded: None,
            article_pages: 0,
        }
    }
    pub fn selected(&self) -> Option<&Row> {
        self.rows.get(self.selected)
    }
    pub(crate) fn ready(&self) -> bool {
        matches!(self.status, Status::Ready | Status::Partial)
    }
    pub(crate) fn restart(&mut self, next: bool) {
        self.after = if next { self.rows.last().map(|r| r.key) } else { None };
        self.rows.clear();
        self.query = None;
        self.more = false;
        self.selected = 0;
        self.reading = false;
        self.loaded = None;
        self.record = None;
        self.name.clear();
        self.text.clear();
        self.status = Status::Loading;
    }
    pub(crate) fn invalidate_selection(&mut self) {
        self.loaded = None;
        self.record = None;
    }
    pub(crate) fn invalidate(&mut self) {
        self.status = Status::Stale;
        self.record = None;
        self.loaded = None;
        self.text.clear();
    }
    pub(crate) fn selection(&self) -> Option<Selection> {
        let row = self.selected()?;
        Some(Selection { qid: row.key.qid, record_index: row.index, map_generation: self.generation? })
    }
    pub(crate) fn read_step(&mut self, reader: &Reader, sources: bool) -> Result<(), Error> {
        if Some(reader.generation()) != self.generation {
            self.invalidate();
            return Ok(());
        }
        let Some(section) = map_section(reader.source())? else {
            self.status = Status::Missing;
            return Ok(());
        };
        let directory = LandmarkDirectory::read(&section)?;
        if self.status == Status::Loading {
            let query = self.query.get_or_insert_with(|| {
                LandmarkQuery::new(directory, reader.generation(), self.origin, RADIUS_M, self.after)
            });
            for _ in 0..64 {
                match query.step(&section, directory, reader.generation(), &mut self.rows) {
                    QueryProgress::Pending => {}
                    QueryProgress::Ready { more } => {
                        self.more = more;
                        self.status = if self.rows.is_empty() { Status::Empty } else { Status::Ready };
                        break;
                    }
                    QueryProgress::Failed(error) => {
                        self.status = Status::Partial;
                        return Err(error);
                    }
                    QueryProgress::Cancelled => {
                        self.invalidate();
                        return Ok(());
                    }
                }
            }
        }
        if !self.ready() {
            return Ok(());
        }
        let Some(row) = self.selected().copied() else {
            return Ok(());
        };
        let requested = (self.selected, sources, if sources { self.source_page } else { self.page });
        if self.loaded == Some(requested) {
            return Ok(());
        }
        let mut record = directory.record(&section, row.index)?;
        if record.qid != row.key.qid {
            self.invalidate();
            return Ok(());
        }
        self.name.clear();
        let mut name = [0; MAX_NAME_BYTES as usize];
        self.name.push_str(directory.name(&section, &record, &mut name)?).map_err(|_| Error::BadOffset)?;
        if !self.name.chars().all(|c| matches!(c,' '..='~'|'\u{a0}'..='\u{17f}')) {
            return Err(Error::BadOffset);
        }
        self.text.clear();
        self.article_pages = credit_count(&directory, &section, record.article)?;
        let photo_credits = if record.photo_attribution.is_absent() {
            None
        } else {
            credit_count(&directory, &section, record.photo_attribution).ok()
        };
        if photo_credits.is_none() {
            record.photo = ContentRef::default();
            record.photo_attribution = ContentRef::default();
        }
        self.source_pages = self.article_pages + photo_credits.unwrap_or(0);
        let reference = if sources {
            if self.source_page < self.article_pages {
                record.article
            } else {
                record.photo_attribution
            }
        } else {
            record.text
        };
        let limit = if sources { MAX_ATTRIBUTION_BYTES } else { MAX_TEXT_BYTES };
        let content = directory.content(&section, reference, limit)?;
        let (count, index) = if sources {
            let mut count = [0; 2];
            content.read_at(0, &mut count).map_err(Error::Source)?;
            (
                rd_u16(&count, 0),
                4 + if self.source_page < self.article_pages {
                    self.source_page
                } else {
                    self.source_page - self.article_pages
                },
            )
        } else {
            (record.text_pages as u16, self.page.min(record.text_pages as u16 - 1))
        };
        let mut bytes = [0; MAX_PAGE_BYTES];
        let text = page(&content, count, index, &mut bytes)?;
        if !display_page(text) {
            return Err(Error::BadOffset);
        }
        self.text.push_str(text).map_err(|_| Error::BadOffset)?;
        self.record = Some(record);
        self.loaded = Some(requested);
        Ok(())
    }
}
impl Default for Landmarks {
    fn default() -> Self {
        Self::new()
    }
}
fn credit_count(directory: &LandmarkDirectory, section: &dyn ByteSource, reference: ContentRef) -> Result<u16, Error> {
    let source = directory.content(section, reference, MAX_ATTRIBUTION_BYTES)?;
    let mut count = [0; 2];
    source.read_at(0, &mut count).map_err(Error::Source)?;
    let count = rd_u16(&count, 0);
    if !(5..=MAX_CREDIT_PAGES + 4).contains(&count) {
        return Err(Error::BadOffset);
    }
    Ok(count - 4)
}
fn display_page(text: &str) -> bool {
    let font = obc_render::text::Font::Label;
    text.lines().count() <= 240 / font.line_height() as usize
        && text.lines().all(|line| line.chars().count() <= 216 / font.char_width() as usize)
        && text.chars().all(|c| matches!(c,'\n'|'\t'|' '..='~'|'\u{a0}'..='\u{17f}'))
}
impl crate::App {
    pub fn open_landmarks(&mut self) {
        self.ui.landmarks = Landmarks::new();
        if let Some(fix) = self.fresh_position() {
            self.ui.landmarks.origin = (fix.lon, fix.lat);
            self.ui.landmarks.status = Status::Loading;
        } else {
            self.ui.landmarks.status = Status::NoFix;
        }
        crate::screen::apply(&mut self.ui.stack, Transition::Push(Screen::Landmarks(crate::screen::LandmarksScreen)));
        self.ui.map_dirty = true;
    }
    pub fn landmarks_status(&self) -> Status {
        self.ui.landmarks.status
    }
    pub fn landmarks_pending(&self) -> bool {
        self.ui.landmarks.status == Status::Loading && matches!(self.ui.stack.last(), Some(Screen::Landmarks(_)))
    }
    pub fn landmark_count(&self) -> usize {
        self.ui.landmarks.rows.len()
    }
    pub fn landmark_qid(&self) -> Option<u64> {
        self.ui.landmarks.selected().map(|r| r.key.qid)
    }
    pub(crate) fn prepare_landmarks(&mut self, reader: Option<&Reader>) {
        let Some(screen) = self.ui.stack.last() else {
            return;
        };
        if !matches!(screen, Screen::Landmarks(_) | Screen::LandmarkSources(_)) {
            return;
        }
        let sources = matches!(screen, Screen::LandmarkSources(_));
        let state = &mut self.ui.landmarks;
        if matches!(
            state.status,
            Status::Stale
                | Status::NoFix
                | Status::NoMap
                | Status::Failed
                | Status::Missing
                | Status::Unsupported
                | Status::Idle
        ) {
            return;
        }
        let Some(reader) = reader else {
            state.status = Status::NoMap;
            return;
        };
        if state.generation.is_none() {
            state.generation = Some(reader.generation());
        }
        let before = state.loaded;
        if let Err(error) = state.read_step(reader, sources) {
            state.status = match error {
                Error::BadVersion => Status::Unsupported,
                _ if state.status == Status::Partial => Status::Partial,
                _ => Status::Failed,
            };
            state.loaded = None;
            state.text.clear();
            state.record = None;
        }
        if state.loaded != before {
            if let Some(record) = state.record {
                let hours = reader.try_poi_hours(record.hours_ref);
                self.ui.poi_scratch.detail_valid = hours.is_ok();
                self.ui.poi_scratch.detail_schedule = hours.ok().flatten();
            }
        }
        if state.status == Status::Loading {
            self.ui.map_dirty = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_formats::{io::SliceSource, obcm};
    use obc_reader::{MapCache, MapTables};
    use std::{vec, vec::Vec};
    fn fields(values: &[&str]) -> Vec<u8> {
        let mut out = (values.len() as u16).to_le_bytes().to_vec();
        let mut off = 2 + (values.len() as u32 + 1) * 4;
        out.extend(off.to_le_bytes());
        for value in values {
            off += value.len() as u32;
            out.extend(off.to_le_bytes());
        }
        for value in values {
            out.extend(value.as_bytes());
        }
        out
    }
    fn map() -> Vec<u8> {
        map_with_credits(&["Credit page one.", "Credit page two."])
    }
    fn map_with_credits(credits: &[&str]) -> Vec<u8> {
        let mut map = obcm_testkit::build_poi_map((0, 0, 1000, 1000), 512, &[]);
        let start = map.len().next_multiple_of(obcm_testkit::UNIT);
        map.resize(start, 0);
        let count = 7;
        let payload = SECTION_HEADER_LEN + count * RECORD_LEN;
        let mut section = vec![0; payload];
        let mut append = |bytes: &[u8]| {
            let reference = ContentRef { offset: section.len() as u32, len: bytes.len() as u32 };
            section.extend(bytes);
            reference
        };
        let name = append("Ruin ä".as_bytes());
        let text = append(&fields(&[
            "First source page.",
            "Second source
page.",
        ]));
        let mut credit_fields = vec!["A", "URL", "License", "License URL"];
        credit_fields.extend_from_slice(credits);
        let article = append(&fields(&credit_fields));
        for i in 0..count {
            let record = LandmarkRecord {
                qid: i as u64 + 1,
                lon: 0,
                lat: 0,
                category: 2,
                language: *b"de",
                text_pages: 2,
                hours_ref: obcm::POI_HOURS_REF_NONE,
                osm: None,
                name,
                text,
                article,
                photo: ContentRef::default(),
                photo_attribution: ContentRef::default(),
            };
            section[SECTION_HEADER_LEN + i * RECORD_LEN..SECTION_HEADER_LEN + (i + 1) * RECORD_LEN]
                .copy_from_slice(&record.encode());
        }
        section[..4].copy_from_slice(&(count as u32).to_le_bytes());
        section[4..6].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
        section[8..12].copy_from_slice(&(payload as u32).to_le_bytes());
        let len = section.len() as u32;
        section[12..16].copy_from_slice(&len.to_le_bytes());
        section.resize(section.len().next_multiple_of(obcm_testkit::UNIT), 0);
        map[obcm::HEADER_LANDMARK_OFFSET_OFF..obcm::HEADER_LANDMARK_OFFSET_OFF + 4]
            .copy_from_slice(&obcm_testkit::scaled(start).to_le_bytes());
        map[obcm::HEADER_LANDMARK_LENGTH_OFF..obcm::HEADER_LANDMARK_LENGTH_OFF + 4]
            .copy_from_slice(&obcm_testkit::scaled(section.len()).to_le_bytes());
        map.extend(section);
        map
    }
    #[test]
    fn colocated_qids_page_completely_and_sources_restore_the_exact_reading_page() {
        let bytes = map();
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut state = Landmarks::new();
        state.generation = Some(reader.generation());
        state.restart(false);
        state.read_step(&reader, false).unwrap();
        assert_eq!(state.rows.iter().map(|r| r.key.qid).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert!(state.more);
        assert_eq!(&*state.name, "Ruin ä");
        state.selected = 2;
        state.reading = true;
        state.page = 1;
        state.read_step(&reader, false).unwrap();
        let before = state.text.clone();
        assert_eq!(
            &*before,
            "Second source
page."
        );
        state.source_page = 1;
        state.read_step(&reader, true).unwrap();
        assert_eq!(&*state.text, "Credit page two.");
        assert_eq!(state.source_pages, 2);
        state.read_step(&reader, false).unwrap();
        assert_eq!(state.text, before);
        assert_eq!(state.selected().unwrap().key.qid, 3);
        state.restart(true);
        state.read_step(&reader, false).unwrap();
        assert_eq!(state.rows.iter().map(|r| r.key.qid).collect::<Vec<_>>(), [5, 6, 7]);
        assert!(!state.more);
        state.generation = Some(reader.generation().wrapping_add(1));
        state.read_step(&reader, false).unwrap();
        assert_eq!(state.status, Status::Stale);
        assert!(state.text.is_empty());
        assert!(state.record.is_none());
        assert!(
            core::mem::size_of::<Landmarks>() < 1800,
            "only four identities, selected name and one page are resident"
        );
    }
    #[test]
    fn full_attribution_budget_keeps_the_last_page_accessible() {
        let pages: Vec<_> = (0..MAX_CREDIT_PAGES).map(|i| std::format!("Credit page {i}")).collect();
        let refs: Vec<_> = pages.iter().map(std::string::String::as_str).collect();
        let bytes = map_with_credits(&refs);
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut state = Landmarks::new();
        state.generation = Some(reader.generation());
        state.restart(false);
        state.read_step(&reader, false).unwrap();
        assert_eq!(state.source_pages, MAX_CREDIT_PAGES);
        state.source_page = MAX_CREDIT_PAGES - 1;
        state.read_step(&reader, true).unwrap();
        assert_eq!(state.text.as_str(), "Credit page 255");
        state.selected = 1;
        state.invalidate_selection();
        state.selected = 0;
        state.invalidate_selection();
        state.read_step(&reader, false).unwrap();
        assert!(state.record.is_some(), "returning selection reloads its identity");
    }
    #[test]
    fn unreadable_photo_credit_does_not_erase_article_text() {
        let mut bytes = map();
        let start = obc_formats::io::rd_u32(&bytes, obcm::HEADER_LANDMARK_OFFSET_OFF) as usize
            * (1 << bytes[obcm::HEADER_OFFSET_SCALE_OFF]);
        let row = start + SECTION_HEADER_LEN;
        let text_offset = obc_formats::io::rd_u32(&bytes, row + 60);
        bytes[row + 76..row + 80].copy_from_slice(&text_offset.to_le_bytes());
        bytes[row + 80..row + 84].copy_from_slice(&4u32.to_le_bytes());
        bytes[row + 84..row + 88].copy_from_slice(&u32::MAX.to_le_bytes());
        bytes[row + 88..row + 92].copy_from_slice(&4u32.to_le_bytes());
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut state = Landmarks::new();
        state.generation = Some(reader.generation());
        state.restart(false);
        state.read_step(&reader, false).unwrap();
        assert_eq!(state.text.as_str(), "First source page.");
        assert!(state.record.unwrap().photo.is_absent());
        state.read_step(&reader, true).unwrap();
        assert_eq!(state.text.as_str(), "Credit page one.");
    }
    #[test]
    fn represented_pages_reject_unsupported_glyphs_and_overflow_without_replacement() {
        assert!(display_page("Français\näöü"));
        assert!(!display_page("雪"));
        assert!(!display_page("1234567890123456789"));
        assert!(!display_page(&"x\n".repeat(11)));
    }
}
