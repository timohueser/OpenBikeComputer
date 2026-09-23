//! Frozen nearby identities and one prepared source page. All map reads happen before draw.
use crate::{
    photo::Selection,
    screen::{Screen, Transition},
};
use obc_formats::{
    io::{ByteSource, WindowSource},
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
    pub peak_source: Option<obc_formats::obcm::SourceId>,
    pub(crate) peak: Option<obc_reader::peaks::Selection>,
    pub(crate) attempted_language: Option<[u8; 2]>,
    pub photo_available: bool,
    pub status: Status,
    pub rows: heapless::Vec<Row, ROWS>,
    pub origin: (i32, i32),
    pub selected: usize,
    pub reading: bool,
    pub page: u16,
    pub source_page: u16,
    pub source_pages: u16,
    /// Whether the loaded Sources screen credits the photo, and that work's first screen.
    pub source_photo: bool,
    pub source_first: u16,
    pub name: heapless::String<256>,
    pub text: heapless::String<MAX_PAGE_BYTES>,
    pub record: Option<LandmarkRecord>,
    pub article: Option<obc_formats::articles::ArticleVariant>,
    pub generation: Option<u32>,
    pub more: bool,
    query: Option<LandmarkQuery>,
    after: Option<LandmarkKey>,
    loaded: Option<(usize, bool, u16, [u8; 2])>,
    article_pages: u16,
}
impl Landmarks {
    pub const fn new() -> Self {
        Self {
            peak_source: None,
            peak: None,
            attempted_language: None,
            photo_available: false,
            status: Status::Idle,
            rows: heapless::Vec::new(),
            origin: (0, 0),
            selected: 0,
            reading: false,
            page: 0,
            source_page: 0,
            source_pages: 0,
            source_photo: false,
            source_first: 0,
            name: heapless::String::new(),
            text: heapless::String::new(),
            record: None,
            article: None,
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
    /// Content is text or a photo. Peak View marks a summit by this, and opens what it names.
    pub(crate) fn has_content(&self) -> bool {
        self.ready() && (self.article.is_some() || self.photo_available)
    }
    pub(crate) fn restart(&mut self, next: bool) {
        self.after = if next { self.rows.last().map(|r| r.key) } else { None };
        self.rows.clear();
        self.query = None;
        self.more = false;
        self.selected = 0;
        self.reading = false;
        self.loaded = None;
        self.photo_available = false;
        self.record = None;
        self.article = None;
        self.name.clear();
        self.text.clear();
        self.status = Status::Loading;
    }
    pub(crate) fn invalidate_selection(&mut self) {
        self.loaded = None;
        self.photo_available = false;
        self.record = None;
        self.article = None;
    }
    pub(crate) fn invalidate(&mut self) {
        self.status = Status::Stale;
        self.photo_available = false;
        self.record = None;
        self.article = None;
        self.loaded = None;
        self.text.clear();
    }
    pub(crate) fn selection(&self) -> Option<crate::photo::ContentSelection> {
        if let Some(peak) = self.peak {
            return Some(crate::photo::ContentSelection::Peak(peak));
        }
        let row = self.selected()?;
        Some(crate::photo::ContentSelection::Landmark(Selection {
            qid: row.key.qid,
            record_index: row.index,
            map_generation: self.generation?,
        }))
    }
    pub(crate) fn read_step(&mut self, reader: &Reader, sources: bool, language: [u8; 2]) -> Result<(), Error> {
        if Some(reader.generation()) != self.generation {
            self.invalidate();
            return Ok(());
        }
        if let Some(selection) = self.peak {
            return reader.with_peak_article(selection, |section, directory, record| {
                let name = directory.content(section, &record, 0, MAX_NAME_BYTES)?;
                let bundle = match record.content[1].is_absent() {
                    true => None,
                    false => Some(directory.content(section, &record, 1, obc_formats::articles::MAX_BYTES)?),
                };
                let photo = directory.content(section, &record, 2, PHOTO_MAX_COMPRESSED as u32).ok();
                let credits = photo.and_then(|_| directory.content(section, &record, 3, MAX_ATTRIBUTION_BYTES).ok());
                self.read_content(
                    &name,
                    bundle.as_ref().map(|s| s as &dyn ByteSource),
                    credits.as_ref().map(|s| s as &dyn ByteSource),
                    sources,
                    language,
                )
            });
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
        let mut record = directory.record(&section, row.index)?;
        if record.qid != row.key.qid {
            self.invalidate();
            return Ok(());
        }
        let name = directory.content(&section, record.name, MAX_NAME_BYTES)?;
        let bundle = directory.content(&section, record.articles, obc_formats::articles::MAX_BYTES)?;
        let photo = directory.content(&section, record.photo, PHOTO_MAX_COMPRESSED as u32).ok();
        let credits =
            photo.and_then(|_| directory.content(&section, record.photo_attribution, MAX_ATTRIBUTION_BYTES).ok());
        self.read_content(&name, Some(&bundle), credits.as_ref().map(|s| s as &dyn ByteSource), sources, language)?;
        if !self.photo_available {
            record.photo = ContentRef::default();
            record.photo_attribution = ContentRef::default();
        }
        self.record = Some(record);
        Ok(())
    }

    fn read_content(
        &mut self,
        name: &dyn ByteSource,
        bundle: Option<&dyn ByteSource>,
        photo_credits: Option<&dyn ByteSource>,
        sources: bool,
        language: [u8; 2],
    ) -> Result<(), Error> {
        if self.loaded.is_some_and(|loaded| loaded.3 != language) {
            self.page = 0;
            self.source_page = 0;
        }
        let requested = (self.selected, sources, if sources { self.source_page } else { self.page }, language);
        if self.loaded == Some(requested) {
            return Ok(());
        }
        let article = bundle.map(|bundle| obc_reader::articles::select(bundle, language)).transpose()?;
        self.name.clear();
        let mut bytes = [0; MAX_NAME_BYTES as usize];
        let bytes = bytes.get_mut(..name.len() as usize).ok_or(Error::BadOffset)?;
        name.read_at(0, bytes).map_err(Error::Source)?;
        self.name.push_str(core::str::from_utf8(bytes).map_err(|_| Error::BadOffset)?).map_err(|_| Error::BadOffset)?;
        if self.name.is_empty() || !self.name.chars().all(|c| matches!(c,' '..='~'|'\u{a0}'..='\u{17f}')) {
            return Err(Error::BadOffset);
        }
        let pages = match bundle.zip(article) {
            Some((bundle, article)) => Some((
                article,
                content(bundle, article.text, MAX_TEXT_BYTES)?,
                content(bundle, article.attribution, MAX_ATTRIBUTION_BYTES)?,
            )),
            None => None,
        };
        // Both credits are read to count the Sources screens; `text` is the scratch until the end.
        let mut bytes = [0; MAX_PAGE_BYTES];
        self.article_pages = match &pages {
            Some((_, _, credits)) => read_credit(credits, false, &mut bytes, &mut self.text)?,
            None => 0,
        };
        let photo_credits = photo_credits.filter(|_| self.loaded.is_none() || self.photo_available);
        let photo_count = photo_credits.and_then(|source| read_credit(source, true, &mut bytes, &mut self.text).ok());
        self.photo_available = photo_count.is_some();
        self.source_pages = self.article_pages + photo_count.unwrap_or(0);
        if self.source_page >= self.source_pages {
            self.source_page = 0;
        }
        self.article = pages.as_ref().map(|(article, _, _)| *article);
        self.text.clear();
        if sources {
            self.source_photo = self.source_page >= self.article_pages;
            self.source_first = if self.source_photo { self.article_pages } else { 0 };
            let credits = match (&pages, photo_credits) {
                (Some((_, _, credits)), _) if !self.source_photo => credits as &dyn ByteSource,
                (_, Some(credits)) if self.photo_available => credits,
                _ => return Err(Error::BadOffset),
            };
            read_credit(credits, self.source_photo, &mut bytes, &mut self.text)?;
        } else if let Some((article, text, _)) = &pages {
            let count = article.text_pages as u16;
            let page = read_display_page(text, count, self.page.min(count - 1), &mut bytes)?;
            self.text.push_str(page).map_err(|_| Error::BadOffset)?;
        } else if !self.photo_available {
            return Err(Error::BadOffset);
        }
        self.loaded = Some((requested.0, sources, if sources { self.source_page } else { self.page }, language));
        Ok(())
    }
}
impl Default for Landmarks {
    fn default() -> Self {
        Self::new()
    }
}
fn content(source: &dyn ByteSource, reference: ContentRef, limit: u32) -> Result<WindowSource<'_>, Error> {
    let range = reference.range(0, source.len() as u32, limit).ok_or(Error::BadOffset)?;
    WindowSource::new(source, range.start, range.end - range.start).ok_or(Error::BadOffset)
}
fn read_display_page<'a>(
    source: &dyn ByteSource,
    count: u16,
    index: u16,
    bytes: &'a mut [u8; MAX_PAGE_BYTES],
) -> Result<&'a str, Error> {
    let text = page(source, count, index, bytes)?;
    if text.trim().is_empty() || !display_page(text) {
        return Err(Error::BadOffset);
    }
    Ok(text)
}
/// Load one work's credit into `out`, its four fields one per line, and count its Sources screens.
/// Only the creator may be empty: a public-domain dedication names none.
fn read_credit(
    source: &dyn ByteSource,
    photo: bool,
    bytes: &mut [u8; MAX_PAGE_BYTES],
    out: &mut heapless::String<MAX_PAGE_BYTES>,
) -> Result<u16, Error> {
    out.clear();
    for index in 0..CREDIT_FIELDS {
        let field = page(source, CREDIT_FIELDS, index, bytes)?;
        if (field.is_empty() && index != 2) || !field.chars().all(|c| matches!(c, ' '..='~' | '\u{a0}'..='\u{17f}')) {
            return Err(Error::BadOffset);
        }
        if index > 0 {
            out.push('\n').map_err(|_| Error::BadOffset)?;
        }
        out.push_str(field).map_err(|_| Error::BadOffset)?;
    }
    Ok(crate::screen::source_layout(photo, out, |_, _, _| {}))
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
        if !matches!(
            screen,
            Screen::Landmarks(_) | Screen::PeakArticle(_) | Screen::LandmarkSources(_) | Screen::LandmarkPhoto(_)
        ) {
            return;
        }
        let sources = matches!(screen, Screen::LandmarkSources(_));
        let language = self.settings().language.article_code();
        // A photo-only record opens its photo straight from Peak View, with no article screen
        // between, so Peak View itself anchors the binding it made.
        let bound = self.ui.landmarks.peak.zip(self.ui.landmarks.peak_source);
        let peak = self
            .ui
            .stack
            .iter()
            .rev()
            .find_map(|screen| match screen {
                Screen::PeakArticle(page) => Some(Some((page.selection, page.source))),
                Screen::PeakView(_) => Some(bound),
                Screen::Landmarks(_) => Some(None),
                _ => None,
            })
            .flatten();
        let fresh_position = self.fresh_position();
        let state = &mut self.ui.landmarks;
        if peak.is_none() && state.peak_source.is_some() {
            *state = Landmarks::new();
            if let Some(fix) = fresh_position {
                state.origin = (fix.lon, fix.lat);
                state.status = Status::Loading;
            } else {
                state.status = Status::NoFix;
            }
        }
        if let Some((selection, source)) = peak.filter(|(selection, _)| state.peak != Some(*selection)) {
            *state = Landmarks::new();
            state.peak = Some(selection);
            state.peak_source = Some(source);
            state.generation = reader.map(Reader::generation);
            state.status = Status::Ready;
            state.reading = true;
        }
        if state.attempted_language != Some(language) {
            state.attempted_language = Some(language);
            if matches!(state.status, Status::Failed | Status::Unsupported) {
                state.status = Status::Ready;
                state.page = 0;
                state.source_page = 0;
                state.invalidate_selection();
            }
        }
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
        if let Err(error) = state.read_step(reader, sources, language) {
            state.status = match error {
                Error::BadVersion => Status::Unsupported,
                _ if state.status == Status::Partial => Status::Partial,
                _ => Status::Failed,
            };
            state.loaded = None;
            state.text.clear();
            state.record = None;
            state.article = None;
        }
        if let Some(record) = state.record {
            let source = record.osm.map_or(0, |m| m.source.0);
            if state.loaded != before || self.ui.poi_scratch.detail_source != source {
                let hours = reader.try_poi_hours(record.hours_ref);
                self.ui.poi_scratch.detail_source = source;
                self.ui.poi_scratch.detail_valid = hours.is_ok();
                self.ui.poi_scratch.detail_schedule = hours.ok().flatten();
            }
        }
        if state.ready() && !state.photo_available {
            for screen in &mut self.ui.stack {
                if let Screen::LandmarkPhoto(photo) = screen {
                    if photo.linked && Some(photo.selection) == state.selection() {
                        photo.invalidate_source();
                    }
                }
            }
        }
        if state.status == Status::Loading {
            self.ui.map_dirty = true;
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
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
    pub(crate) const CREDIT: [&str; 4] =
        ["de.wikipedia.org/?oldid=1", "Ruine", "Wikipedia contributors", "CC BY-SA 4.0"];
    /// A map with one landmark section: seven records with article text, credits and no photo.
    /// The copy-fit gate renders the reading page over it, so the builder is crate-visible.
    pub(crate) fn map() -> Vec<u8> {
        map_with_credits(&CREDIT)
    }
    fn map_with_credits(credits: &[&str]) -> Vec<u8> {
        map_with_photo_credits(credits, &[])
    }
    fn map_with_photo_credits(credits: &[&str], photo_credits: &[&str]) -> Vec<u8> {
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
        let text = &["First source page.", "Second source\npage."];
        let spanish = ["es.wikipedia.org/?oldid=42", "Ruina", "Wikipedia contributors", "CC BY-SA 4.0"];
        let articles = append(&obcm_testkit::articles::bundle(
            *b"de",
            &[(*b"de", text, credits), (*b"es", &["Una ruina."], &spanish)],
        ));
        let (photo, photo_attribution) = if photo_credits.is_empty() {
            (ContentRef::default(), ContentRef::default())
        } else {
            (append(&[0; 4]), append(&fields(photo_credits)))
        };
        for i in 0..count {
            let record = LandmarkRecord {
                qid: i as u64 + 1,
                lon: 0,
                lat: 0,
                category: 2,
                hours_ref: obcm::POI_HOURS_REF_NONE,
                osm: None,
                name,
                articles,
                photo,
                photo_attribution,
            };
            section[SECTION_HEADER_LEN + i * RECORD_LEN..SECTION_HEADER_LEN + (i + 1) * RECORD_LEN]
                .copy_from_slice(&record.encode());
        }
        section[..4].copy_from_slice(&(count as u32).to_le_bytes());
        section[4..6].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
        section[6..8].copy_from_slice(&SECTION_VERSION.to_le_bytes());
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
        state.read_step(&reader, false, *b"en").unwrap();
        assert_eq!(state.rows.iter().map(|r| r.key.qid).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert!(state.more);
        assert_eq!(&*state.name, "Ruin ä");
        state.selected = 2;
        state.reading = true;
        state.page = 1;
        state.read_step(&reader, false, *b"en").unwrap();
        let before = state.text.clone();
        assert_eq!(
            &*before,
            "Second source
page."
        );
        state.read_step(&reader, true, *b"en").unwrap();
        assert_eq!(state.text.as_str(), CREDIT.join("\n"));
        assert_eq!(state.source_pages, 1);
        state.read_step(&reader, false, *b"en").unwrap();
        assert_eq!(state.text, before);
        assert_eq!(state.selected().unwrap().key.qid, 3);
        state.restart(true);
        state.read_step(&reader, false, *b"en").unwrap();
        assert_eq!(state.rows.iter().map(|r| r.key.qid).collect::<Vec<_>>(), [5, 6, 7]);
        assert!(!state.more);
        state.generation = Some(reader.generation().wrapping_add(1));
        state.read_step(&reader, false, *b"en").unwrap();
        assert_eq!(state.status, Status::Stale);
        assert!(state.text.is_empty());
        assert!(state.record.is_none());
        assert!(
            core::mem::size_of::<Landmarks>() < 1800,
            "only four identities, selected name and one page are resident"
        );
    }
    #[test]
    fn changing_ui_language_reloads_text_and_credits_and_resets_page() {
        let bytes = map();
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut state = Landmarks::new();
        state.generation = Some(reader.generation());
        state.restart(false);
        state.read_step(&reader, false, *b"de").unwrap();
        state.page = 1;
        state.read_step(&reader, false, *b"de").unwrap();
        state.read_step(&reader, false, *b"es").unwrap();
        assert_eq!(state.page, 0);
        assert_eq!(state.text.as_str(), "Una ruina.");
        assert_eq!(state.article.unwrap().language, *b"es");
        state.read_step(&reader, true, *b"es").unwrap();
        assert!(state.text.starts_with("es.wikipedia.org/?oldid=42\nRuina\n"));
        state.read_step(&reader, false, *b"fr").unwrap();
        assert_eq!(state.article.unwrap().language, *b"de", "no English: use the baked default");
        assert_eq!(state.text.as_str(), "First source page.");
    }
    #[test]
    fn a_long_photo_credit_continues_on_its_own_screens_after_the_text() {
        let creator = "Name ".repeat(60);
        let photo = ["Wikimedia Commons", "Ruin.jpg", creator.trim_end(), "CC BY 4.0"];
        let bytes = map_with_photo_credits(&CREDIT, &photo);
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut state = Landmarks::new();
        state.generation = Some(reader.generation());
        state.restart(false);
        state.read_step(&reader, false, *b"en").unwrap();
        let photo_screens = crate::screen::source_layout(true, &photo.join("\n"), |_, _, _| {});
        assert!(photo_screens > 1);
        assert_eq!(state.source_pages, 1 + photo_screens, "the text credit fits one screen");
        state.source_page = state.source_pages - 1;
        state.read_step(&reader, true, *b"en").unwrap();
        assert!(state.source_photo);
        assert_eq!((state.source_first, state.text.as_str()), (1, photo.join("\n").as_str()));
        state.source_page = 0;
        state.read_step(&reader, true, *b"en").unwrap();
        assert_eq!((state.source_photo, state.source_first), (false, 0));
        assert_eq!(state.text.as_str(), CREDIT.join("\n"));
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
        state.read_step(&reader, false, *b"en").unwrap();
        assert_eq!(state.text.as_str(), "First source page.");
        assert!(state.record.unwrap().photo.is_absent());
        state.read_step(&reader, true, *b"en").unwrap();
        assert_eq!(state.text.as_str(), CREDIT.join("\n"));
    }
    #[test]
    fn unreadable_photo_credit_drops_the_photo_and_keeps_the_article_sources() {
        let bytes = map_with_photo_credits(&CREDIT, &["Wikimedia Commons", "雪.jpg", "A", "CC BY 4.0"]);
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
        assert!(app.ui.stack.push(Screen::Landmarks(crate::screen::LandmarksScreen)).is_ok());
        app.ui.landmarks.restart(false);
        app.prepare_landmarks(Some(&reader));
        app.ui.landmarks.reading = true;
        app.ui.landmarks.page = 1;
        app.prepare_landmarks(Some(&reader));
        assert!(app.ui.landmarks.record.unwrap().photo.is_absent(), "an unreadable credit drops its photo");
        assert!(app.ui.stack.push(Screen::LandmarkSources(crate::screen::LandmarkSourcesScreen)).is_ok());
        app.ui.landmarks.source_page = 3;
        app.prepare_landmarks(Some(&reader));
        assert_eq!(app.ui.landmarks.status, Status::Ready);
        assert_eq!(app.ui.landmarks.text.as_str(), CREDIT.join("\n"));
        assert_eq!((app.ui.landmarks.source_page, app.ui.landmarks.source_pages), (0, 1));
        assert!(app.ui.landmarks.record.unwrap().photo.is_absent());
        app.apply_gesture(crate::Gesture::Back);
        app.prepare_landmarks(Some(&reader));
        assert_eq!(app.ui.landmarks.page, 1);
        assert_eq!(app.ui.landmarks.text.as_str(), "Second source\npage.");
        assert!(app.ui.landmarks.record.unwrap().photo.is_absent(), "failed optional credit stays omitted");
    }
    #[test]
    fn populated_failed_or_stale_card_refreshes_on_press() {
        let bytes = map();
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        for failed in [false, true] {
            let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
            app.bind_place_map(Some(obc_formats::obcr::RouteSourceKey { store: [1; 16], object: 1, revision: 1 }));
            assert!(app.ui.stack.push(Screen::Landmarks(crate::screen::LandmarksScreen)).is_ok());
            app.ui.landmarks.restart(false);
            app.prepare_landmarks(Some(&reader));
            assert_eq!(app.ui.landmarks.rows.len(), 4);
            if failed {
                app.ui.landmarks.status = Status::Failed;
                app.ui.landmarks.invalidate_selection();
            } else {
                app.bind_place_map(Some(obc_formats::obcr::RouteSourceKey { store: [1; 16], object: 1, revision: 2 }));
                assert_eq!(app.ui.landmarks.status, Status::Stale);
            }
            app.state.user_fix = Some(obc_ports::Fix::at(0, 0));
            app.apply_gesture(crate::Gesture::Press);
            assert_eq!(app.ui.landmarks.status, Status::Loading);
            app.prepare_landmarks(Some(&reader));
            assert_eq!(app.ui.landmarks.status, Status::Ready);
            assert!(app.ui.landmarks.record.is_some());
        }
    }
    #[test]
    fn landmark_hours_replace_cache_ownership_and_retained_details_reprepare() {
        let bytes = map();
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&source, &tables, &cache);
        let mut app = crate::App::new_idle(crate::AppState::new(0, 0, 1.0));
        let poi = obc_reader::Poi {
            opening: obc_reader::hours::OpeningStatus::Unknown,
            metadata: obcm::PoiMetadata { source: obcm::SourceId(42), approach: None },
            lon: 0,
            lat: 0,
            subtype: 1,
            name: heapless::String::new(),
            hours_ref: obcm::POI_HOURS_REF_NONE,
            distance_m: 0,
        };
        assert!(app.ui.stack.push(Screen::PoiDetail(crate::screen::PoiDetailScreen::new(poi))).is_ok());
        let mut frame = crate::harness::support::Buf::new(240, 320);
        app.render_frame(None, &mut frame, &reader, None, 240.0, 320.0, |color| {
            let (r, g, b) = obc_reader::rgb565_to_rgb888(color);
            embedded_graphics::pixelcolor::Rgb888::new(r, g, b)
        });
        assert_eq!(app.ui.poi_scratch.detail_source, 42);
        assert!(app.ui.stack.push(Screen::Landmarks(crate::screen::LandmarksScreen)).is_ok());
        app.ui.landmarks.restart(false);
        app.prepare_landmarks(Some(&reader));
        assert_eq!(app.ui.poi_scratch.detail_source, 0, "information-only article owns its own cached hours");
        app.ui.poi_scratch.detail_source = 42;
        app.ui.poi_scratch.detail_valid = false;
        app.prepare_landmarks(Some(&reader));
        assert_eq!(app.ui.poi_scratch.detail_source, 0, "unchanged article reloads an overwritten cache");
        assert!(app.ui.poi_scratch.detail_valid);
        app.apply_gesture(crate::Gesture::Back);
        assert!(matches!(app.top_screen(), Screen::PoiDetail(detail) if detail.hours_pending(&app.ui.poi_scratch)));
        app.apply_gesture(crate::Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::PoiDetail(_)), "another site's cached hours cannot activate Visit");
    }
    #[test]
    fn represented_pages_reject_unsupported_glyphs_and_overflow_without_replacement() {
        assert!(display_page("Français\näöü"));
        assert!(!display_page("雪"));
        assert!(!display_page("1234567890123456789"));
        assert!(!display_page(&"x\n".repeat(11)));
    }
}
