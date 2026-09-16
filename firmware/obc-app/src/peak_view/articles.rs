//! Prepare only the selected summit, using the installed map's exact association.
use crate::{
    landmarks::{Landmarks, Status},
    screen::Screen,
    App,
};
use obc_reader::Reader;

impl App {
    pub(crate) fn prepare_peak_article(&mut self, reader: Option<&Reader>) {
        let language = self.settings().language.article_code();
        let Some(Screen::PeakView(screen)) = self.ui.stack.last_mut() else { return };
        let generation = reader.map(Reader::generation);
        if screen.map_generation.is_some() && screen.map_generation != generation {
            screen.invalidate_map();
            self.state.peak_view_peak_count = 0;
        }
        screen.map_generation = generation;
        let Some(source) = screen.selected_source() else { return };
        let state = &mut self.ui.landmarks;
        if state.peak_source != Some(source)
            || state.generation != generation
            || state.attempted_language != Some(language)
        {
            *state = Landmarks::new();
            state.peak_source = Some(source);
            state.attempted_language = Some(language);
            state.generation = generation;
            state.status = Status::Missing;
            let Some(reader) = reader else { return };
            state.peak = source.is_valid().then(|| reader.peak_article(source).ok().flatten()).flatten();
            if state.peak.is_some() {
                state.status = Status::Ready;
            }
        }
        if state.ready() {
            // The indicator requires readable text, not just a valid directory entry.
            state.page = 0;
            if reader.is_none_or(|reader| state.read_step(reader, false, language).is_err()) {
                state.invalidate();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        peak_view::{PeakName, PeakViewPeak, PeakViewProfile},
        Chord, Gesture,
    };
    use obc_formats::{
        io::SliceSource,
        obcm::{self, landmarks::*, peaks::*, SourceId},
    };
    use obc_reader::{MapCache, MapTables};
    use std::{vec, vec::Vec};

    fn fields(values: &[&str]) -> Vec<u8> {
        let mut bytes = (values.len() as u16).to_le_bytes().to_vec();
        let mut offset = 2 + (values.len() as u32 + 1) * 4;
        bytes.extend(offset.to_le_bytes());
        for value in values {
            offset += value.len() as u32;
            bytes.extend(offset.to_le_bytes());
        }
        for value in values {
            bytes.extend(value.as_bytes());
        }
        bytes
    }
    fn map(photo: Option<bool>, text: &str, english: bool) -> Vec<u8> {
        use obcm::peaks::{HEADER_LEN, RECORD_LEN, VERSION};
        let mut section = vec![0; HEADER_LEN + 2 * ASSOCIATION_LEN + RECORD_LEN];
        let payload = section.len() as u32;
        let mut record = Record { id: [1; 32], content: [ContentRef::default(); 4] };
        let credits = ["url", "rev", "license", "authors", "Source credit."];
        let local_credits = ["url", "rev", "license", "authors", if text == "雪" { "雪" } else { "Source credit." }];
        let variants: &[obcm_testkit::articles::Article<'_>] = if english {
            &[
                (*b"de", &[text], &local_credits),
                (*b"en", &["English summit.", "Second page."], &credits),
                (*b"fr", &["Sommet français."], &credits),
                (*b"es", &["Cumbre española."], &credits),
            ]
        } else {
            &[(*b"de", &[text], &local_credits)]
        };
        let mut pixels = vec![0x48, 0x0d, 1];
        let len = PHOTO_PIXELS as u16;
        pixels.extend(len.to_le_bytes());
        pixels.extend((!len).to_le_bytes());
        pixels.resize(7 + PHOTO_PIXELS, 0);
        pixels.extend((((PHOTO_PIXELS as u32 % 65521) << 16) | 1).to_be_bytes());
        if photo == Some(false) {
            pixels[0] = 0;
        }
        let values = [b"Summit".to_vec(), obcm_testkit::articles::bundle(*b"de", variants), pixels, fields(&credits)];
        for (slot, bytes) in values.iter().enumerate() {
            if slot >= 2 && photo.is_none() {
                continue;
            }
            record.content[slot] =
                ContentRef { offset: section.len() as u32, len: bytes.len() as u32 + CONTENT_GUARD_LEN };
            section.extend(record.id);
            section.push(slot as u8);
            section.extend(bytes);
        }
        section[..2].copy_from_slice(&VERSION.to_le_bytes());
        section[2..4].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
        section[4..8].copy_from_slice(&2u32.to_le_bytes());
        section[8..12].copy_from_slice(&1u32.to_le_bytes());
        section[12..16].copy_from_slice(&payload.to_le_bytes());
        let len = section.len() as u32;
        section[16..20].copy_from_slice(&len.to_le_bytes());
        for i in 0..2 {
            let start = HEADER_LEN + i * ASSOCIATION_LEN;
            section[start..start + ASSOCIATION_LEN].copy_from_slice(
                &Association { source: SourceId::osm(1, 101 + i as u64), article: record.id, index: 0 }.encode(),
            );
        }
        section[HEADER_LEN + 2 * ASSOCIATION_LEN..payload as usize].copy_from_slice(&record.encode());
        let mut map = obcm_testkit::build_poi_map((0, 0, 1000, 1000), 512, &[]);
        let start = obcm_testkit::align_up(map.len());
        map.resize(start, 0);
        map.extend(section);
        map.resize(obcm_testkit::align_up(map.len()), 0);
        let len = map.len() - start;
        map[obcm::HEADER_PEAK_OFFSET_OFF..obcm::HEADER_PEAK_OFFSET_OFF + 4]
            .copy_from_slice(&obcm_testkit::scaled(start).to_le_bytes());
        map[obcm::HEADER_PEAK_LENGTH_OFF..obcm::HEADER_PEAK_LENGTH_OFF + 4]
            .copy_from_slice(&obcm_testkit::scaled(len).to_le_bytes());
        map
    }
    fn app() -> App {
        let mut app = App::new_idle(crate::AppState::new(0, 0, 1.0));
        app.state.peak_view_profile = Some(PeakViewProfile::at(200, 300, 0));
        for i in 0..3 {
            app.state.peak_view_peaks[i] = PeakViewPeak {
                source: SourceId::osm(1, 101 + i as u64),
                name: PeakName::new("Summit"),
                lat: 100,
                lon: 100,
                distance_m: 50_000,
                visible: true,
                score: 3 - i as u32,
                ..PeakViewPeak::EMPTY
            };
        }
        app.state.peak_view_peak_count = 3;
        app.state.user_fix = Some(obc_ports::Fix::at(200, 300));
        app.show_peak_view();
        app.set_peak_view_status(crate::peak_view::runtime::Status::Ready);
        app
    }
    fn source(app: &App) -> Option<SourceId> {
        match app.top_screen() {
            Screen::PeakView(peak) => peak.selected_source(),
            _ => panic!("Peak View"),
        }
    }
    fn prepare(app: &mut App, reader: &Reader) {
        app.prepare_peak_article(Some(reader));
        app.prepare_landmarks(Some(reader));
    }
    fn decode(app: &mut App, reader: &Reader) {
        let mut frame = crate::harness::support::Buf::new(240, 320);
        let mut photo = crate::photo::Runtime::new();
        app.render_scene_map_photo_timed(
            None,
            &mut frame,
            Some(reader),
            Some(reader),
            None,
            None,
            240.0,
            320.0,
            |c| {
                let (r, g, b) = obc_reader::rgb565_to_rgb888(c);
                embedded_graphics::pixelcolor::Rgb888::new(r, g, b)
            },
            &obc_render::NoopClock,
            Some(crate::photo::FramePhoto::capture(&mut photo)),
        );
    }
    #[test]
    fn peak_reading_photo_sources_and_back_keep_identity_and_browse_view() {
        let bytes = map(Some(true), "Deutscher Gipfel.", true);
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut app = app();
        assert!(obc_reader::landmarks::map_section(&src).unwrap().is_none());
        assert_eq!(source(&app), None);
        app.apply_gesture(Gesture::Press);
        prepare(&mut app, &reader);
        assert_eq!(source(&app), Some(SourceId::osm(1, 101)));
        assert!(app.ui.landmarks.ready());
        assert!(app.ui.landmarks.rows.is_empty());
        assert_eq!(app.ui.landmarks.text.as_str(), "English summit.");
        for (language, code) in [
            (crate::settings::Language::De, *b"de"),
            (crate::settings::Language::Fr, *b"fr"),
            (crate::settings::Language::Es, *b"es"),
            (crate::settings::Language::En, *b"en"),
        ] {
            let mut settings = *app.settings();
            settings.language = language;
            app.set_settings(settings);
            prepare(&mut app, &reader);
            assert_eq!(app.ui.landmarks.article.unwrap().language, code);
        }
        let heading = app.peak_view_heading_q4();
        app.apply_gesture(Gesture::Press);
        prepare(&mut app, &reader);
        assert!(matches!(app.top_screen(), Screen::PeakArticle(_)));
        app.apply_gesture(Gesture::Step(1));
        prepare(&mut app, &reader);
        assert_eq!(app.ui.landmarks.text.as_str(), "Second page.");
        app.apply_gesture(Gesture::Step(1));
        decode(&mut app, &reader);
        assert_eq!(app.photo_status(), Some(crate::photo::Status::Complete));
        assert!(app.apply_chord(Chord::Context));
        app.apply_gesture(Gesture::Press);
        prepare(&mut app, &reader);
        assert!(matches!(app.top_screen(), Screen::LandmarkSources(_)));
        assert_eq!(app.ui.landmarks.text.as_str(), "Source credit.");
        app.apply_gesture(Gesture::Back);
        decode(&mut app, &reader);
        assert_eq!(app.photo_status(), Some(crate::photo::Status::Complete));
        app.apply_gesture(Gesture::Press);
        prepare(&mut app, &reader);
        assert!(matches!(app.top_screen(), Screen::PeakArticle(_)), "photo Select cannot Visit");
        app.open_landmarks();
        prepare(&mut app, &reader);
        assert!(app.ui.landmarks.peak.is_none(), "normal landmark entry cannot retain peak content");
        app.apply_gesture(Gesture::Back);
        prepare(&mut app, &reader);
        assert_eq!(
            app.ui.landmarks.text.as_str(),
            "English summit.",
            "article restores its own identity after another reader used the buffer"
        );
        app.state.user_fix = Some(obc_ports::Fix::at(10_000, 20_000));
        app.apply_gesture(Gesture::Back);
        prepare(&mut app, &reader);
        assert_eq!(source(&app), Some(SourceId::osm(1, 101)));
        assert_eq!(app.peak_view_heading_q4(), heading);
        assert_eq!(app.peak_view_position(), Some((200, 300)));
        app.apply_gesture(Gesture::Step(1));
        prepare(&mut app, &reader);
        assert_eq!(source(&app), Some(SourceId::osm(1, 102)), "co-located summits remain distinct");
        app.apply_gesture(Gesture::Step(1));
        prepare(&mut app, &reader);
        assert_eq!(source(&app), Some(SourceId::osm(1, 103)));
        assert!(!app.ui.landmarks.ready());
        app.apply_gesture(Gesture::Press);
        assert_eq!(source(&app), None, "unlinked Select returns Live");
        app.apply_gesture(Gesture::Press);
        app.apply_gesture(Gesture::Back);
        assert_eq!(source(&app), None);
        app.apply_gesture(Gesture::Back);
        assert!(!matches!(app.top_screen(), Screen::PeakView(_)));
    }
    #[test]
    fn text_retains_the_panorama_and_photo_reclaims_its_arena() {
        use crate::{
            arena_gate::{ArenaGate, ArenaOwner},
            peak_view::runtime::{Failed, Lifecycle, Platform, Progress},
        };
        use std::cell::RefCell;
        struct Job<'a> {
            gate: &'a RefCell<ArenaGate>,
            starts: Vec<(i32, i32)>,
            peaks: [PeakViewPeak; 3],
            steps: u8,
            complete: bool,
        }
        impl Platform for Job<'_> {
            fn start(&mut self, _: &mut App, position: (i32, i32)) -> bool {
                self.gate.borrow_mut().claim_peak_view().unwrap();
                self.starts.push(position);
                true
            }
            fn step(&mut self, app: &mut App) -> Result<Progress, Failed> {
                self.steps += 1;
                app.state.peak_view_peaks[..3].copy_from_slice(&self.peaks);
                app.state.peak_view_peak_count = 3;
                Ok(Progress { complete: self.complete, revision: 1 })
            }
            fn cancel(&mut self) {
                let mut gate = self.gate.borrow_mut();
                if gate.owner() == ArenaOwner::PeakView {
                    gate.release(ArenaOwner::PeakView).unwrap();
                }
            }
        }
        let bytes = map(Some(true), "Berg.", true);
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut app = app();
        let gate = RefCell::new(ArenaGate::new());
        let mut job = Job {
            gate: &gate,
            starts: Vec::new(),
            peaks: app.state.peak_view_peaks[..3].try_into().unwrap(),
            steps: 0,
            complete: false,
        };
        let mut lifecycle = Lifecycle::default();
        lifecycle.update(&mut app, &mut job, 0);
        app.apply_gesture(Gesture::Press);
        prepare(&mut app, &reader);
        app.apply_gesture(Gesture::Press);
        lifecycle.update(&mut app, &mut job, 10);
        assert!(!app.peak_view_is_base(), "text keeps its own screen and sensor semantics");
        assert_eq!(gate.borrow().owner(), ArenaOwner::PeakView);
        assert!(!lifecycle.busy(), "covered terrain work is paused");
        assert!(app.apply_chord(Chord::Context));
        lifecycle.update(&mut app, &mut job, 11);
        app.apply_gesture(Gesture::Press);
        prepare(&mut app, &reader);
        assert!(matches!(app.top_screen(), Screen::LandmarkSources(_)));
        lifecycle.update(&mut app, &mut job, 12);
        assert_eq!(gate.borrow().owner(), ArenaOwner::PeakView, "text credits retain the panorama");
        assert_eq!(job.steps, 1, "covered text and drawers do not advance terrain work");
        app.apply_gesture(Gesture::Back);
        app.apply_gesture(Gesture::Back);
        job.complete = true;
        lifecycle.update(&mut app, &mut job, 13);
        assert_eq!(job.starts, [(200, 300)], "text and Sources return without another terrain job");
        prepare(&mut app, &reader);
        app.apply_gesture(Gesture::Press);
        prepare(&mut app, &reader);
        app.apply_gesture(Gesture::Step(-1));
        lifecycle.update(&mut app, &mut job, 14);
        assert_ne!(gate.borrow().owner(), ArenaOwner::PeakView, "Photo releases the shared arena");
        gate.borrow_mut().claim_photo().unwrap();
        decode(&mut app, &reader);
        assert!(app.apply_chord(Chord::Context));
        app.apply_gesture(Gesture::Press);
        lifecycle.update(&mut app, &mut job, 15);
        assert!(!app.peak_view_retains_panorama(), "credits over Photo cannot retain the panorama");
        app.apply_gesture(Gesture::Back);
        gate.borrow_mut().release(ArenaOwner::Photo).unwrap();
        app.state.user_fix = Some(obc_ports::Fix::at(40_000, 50_000));
        app.apply_gesture(Gesture::Back);
        app.apply_gesture(Gesture::Back);
        lifecycle.update(&mut app, &mut job, 20);
        prepare(&mut app, &reader);
        assert_eq!(gate.borrow().owner(), ArenaOwner::PeakView);
        assert_eq!(job.starts, [(200, 300), (200, 300)]);
        assert_eq!(source(&app), Some(SourceId::osm(1, 101)));
        app.apply_gesture(Gesture::Press);
        app.open_landmarks();
        lifecycle.update(&mut app, &mut job, 21);
        assert_ne!(gate.borrow().owner(), ArenaOwner::PeakView, "an unrelated screen releases storage");
        app.apply_gesture(Gesture::Back);
        app.apply_gesture(Gesture::Back);
        lifecycle.update(&mut app, &mut job, 22);
        assert_eq!(job.starts.len(), 3);
        for screen in [
            Screen::NavPlanning(crate::screen::NavPlanningScreen::new("Route")),
            Screen::MapTransfer(crate::screen::MapTransferScreen::new(crate::screen::MapTransfer::Receiving {
                received_kib: 0,
                total_kib: 1,
            })),
        ] {
            assert!(app.ui.stack.push(screen).is_ok());
            lifecycle.update(&mut app, &mut job, 23);
            assert_ne!(gate.borrow().owner(), ArenaOwner::PeakView, "navigation and USB need the arena");
            app.ui.stack.pop();
            lifecycle.update(&mut app, &mut job, 24);
            assert_eq!(gate.borrow().owner(), ArenaOwner::PeakView);
        }
        app.apply_gesture(Gesture::Back);
        lifecycle.update(&mut app, &mut job, 30);
        assert_eq!(job.starts.last(), Some(&(40_000, 50_000)), "Live resumes the current fix");
    }

    #[test]
    fn text_only_language_fallback_unreadable_content_and_map_replacement() {
        for photo in [None, Some(false)] {
            let bytes = map(photo, "Deutscher Gipfel.", false);
            let src = SliceSource(&bytes);
            let tables = MapTables::parse(&src).unwrap();
            let cache = MapCache::new();
            let reader = Reader::new(&src, &tables, &cache);
            let mut app = app();
            app.apply_gesture(Gesture::Press);
            prepare(&mut app, &reader);
            assert_eq!(app.ui.landmarks.article.unwrap().language, *b"de", "baked local fallback");
            app.apply_gesture(Gesture::Press);
            app.apply_gesture(Gesture::Step(1));
            prepare(&mut app, &reader);
            if photo.is_some() {
                decode(&mut app, &reader);
                assert_eq!(app.photo_status(), Some(crate::photo::Status::Unavailable));
                app.apply_gesture(Gesture::Back);
            }
            assert!(matches!(app.top_screen(), Screen::PeakArticle(_)));
            prepare(&mut app, &reader);
            assert_eq!(app.ui.landmarks.text.as_str(), "Deutscher Gipfel.");
            let replacement = MapTables::parse(&src).unwrap();
            let changed = Reader::new(&src, &replacement, &cache);
            prepare(&mut app, &changed);
            assert!(!app.ui.landmarks.ready());
            assert!(app.ui.landmarks.text.is_empty());
            app.apply_gesture(Gesture::Back);
            prepare(&mut app, &changed);
            assert_eq!(source(&app), None);
        }
        let bytes = map(None, "雪", true);
        let src = SliceSource(&bytes);
        let tables = MapTables::parse(&src).unwrap();
        let cache = MapCache::new();
        let reader = Reader::new(&src, &tables, &cache);
        let mut app = app();
        let mut settings = *app.settings();
        settings.language = crate::settings::Language::De;
        app.set_settings(settings);
        app.apply_gesture(Gesture::Press);
        prepare(&mut app, &reader);
        assert!(!app.ui.landmarks.ready());
        settings.language = crate::settings::Language::En;
        app.set_settings(settings);
        prepare(&mut app, &reader);
        assert!(app.ui.landmarks.ready(), "changing language retries the selected article");
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::PeakArticle(_)));
        for sources in [false, true] {
            if sources {
                app.apply_chord(Chord::Context);
                app.apply_gesture(Gesture::Press);
            }
            settings.language = crate::settings::Language::De;
            app.set_settings(settings);
            prepare(&mut app, &reader);
            assert_eq!(app.ui.landmarks.status, Status::Failed);
            prepare(&mut app, &reader);
            assert_eq!(app.ui.landmarks.status, Status::Failed, "the same language keeps the failure latched");
            settings.language = crate::settings::Language::En;
            app.set_settings(settings);
            prepare(&mut app, &reader);
            assert!(app.ui.landmarks.ready(), "reading and Sources retry after a language change");
            if sources {
                app.apply_gesture(Gesture::Back);
            }
        }
        app.apply_gesture(Gesture::Back);
        settings.language = crate::settings::Language::De;
        app.set_settings(settings);
        prepare(&mut app, &reader);
        assert!(!app.ui.landmarks.ready());
        app.apply_gesture(Gesture::Press);
        assert!(matches!(app.top_screen(), Screen::PeakView(_)));
    }
}
