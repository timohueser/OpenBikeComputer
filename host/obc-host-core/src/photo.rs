//! Host scratch ownership for the same bounded phase used by the board.
use embedded_graphics::draw_target::DrawTarget;
use obc_app::{photo::Runtime, App};
use obc_reader::Reader;

pub struct Preparer {
    runtime: Box<Runtime>,
}
impl Default for Preparer {
    fn default() -> Self {
        let mut runtime = Box::<Runtime>::new_uninit();
        // SAFETY: this allocation is aligned and exclusively owned; initialization
        // writes every field before assume_init creates the first reference.
        let runtime = unsafe {
            Runtime::init_in_place(runtime.as_mut_ptr());
            runtime.assume_init()
        };
        Self { runtime }
    }
}
impl Preparer {
    pub fn step<D, F>(&mut self, app: &mut App, reader: Option<&Reader<'_>>, target: &mut D, color: F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        app.prepare_photo_step(&mut self.runtime, reader, target, color);
    }

    /// A fresh headless frame runs ordinary steps until it reaches a terminal state.
    /// Every pending decoder step consumes input or produces output, so this bound
    /// covers the format's maximum input and output without an unbounded loop.
    pub fn finish<D, F>(&mut self, app: &mut App, reader: Option<&Reader<'_>>, target: &mut D, color: F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use obc_formats::obcm::landmarks::{PHOTO_MAX_COMPRESSED, PHOTO_PIXELS};
        for _ in 0..=PHOTO_MAX_COMPRESSED + PHOTO_PIXELS {
            if !app.photo_pending() {
                break;
            }
            self.step(app, reader, target, &color);
        }
        debug_assert!(!app.photo_pending());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RgbaFrame;
    use embedded_graphics::pixelcolor::{raw::RawU16, Rgb565, Rgb888};
    use obc_app::{
        photo::{Selection, Status},
        AppState, Chord, Gesture,
    };
    use obc_formats::{
        io::{ByteSource, Error, SliceSource},
        obcm::{self, landmarks::*},
    };
    use obc_reader::{MapCache, MapTables};
    use std::cell::Cell;

    fn fields(values: &[&str]) -> Vec<u8> {
        let mut bytes = (values.len() as u16).to_le_bytes().to_vec();
        let mut offset = 2 + (values.len() as u32 + 1) * 4;
        bytes.extend_from_slice(&offset.to_le_bytes());
        for value in values {
            offset += value.len() as u32;
            bytes.extend_from_slice(&offset.to_le_bytes());
        }
        for value in values {
            bytes.extend_from_slice(value.as_bytes());
        }
        bytes
    }

    fn map(photo: bool) -> Vec<u8> {
        let mut map = obcm_testkit::build_poi_map((0, 0, 1000, 1000), 512, &[]);
        let start = map.len().next_multiple_of(obcm_testkit::UNIT);
        map.resize(start, 0);
        let mut section = vec![0; SECTION_HEADER_LEN + RECORD_LEN];
        let mut append = |bytes: &[u8]| {
            let reference = ContentRef { offset: section.len() as u32, len: bytes.len() as u32 };
            section.extend_from_slice(bytes);
            reference
        };
        let name = append(b"Authored photo");
        let text = append(&fields(&["A test image made for this test."]));
        let credit = fields(&[
            "Test author",
            "https://example.org/test",
            "CC0",
            "https://creativecommons.org/publicdomain/zero/1.0/",
            "Test author. CC0.",
        ]);
        let article = append(&credit);
        // Independently authored zlib: 4 KiB window, one stored DEFLATE block,
        // and an Adler-32 over a pattern containing every RGB222 value.
        let pixels: Vec<u8> = (0..PHOTO_PIXELS).map(|i| (i % 64) as u8).collect();
        let mut compressed = vec![0x48, 0x0d, 1];
        let len = pixels.len() as u16;
        compressed.extend_from_slice(&len.to_le_bytes());
        compressed.extend_from_slice(&(!len).to_le_bytes());
        compressed.extend_from_slice(&pixels);
        let (mut a, mut b) = (1u32, 0u32);
        for pixel in pixels {
            a = (a + u32::from(pixel)) % 65521;
            b = (b + a) % 65521;
        }
        compressed.extend_from_slice(&((b << 16) | a).to_be_bytes());
        let (photo, photo_attribution) =
            if photo { (append(&compressed), append(&credit)) } else { (ContentRef::default(), ContentRef::default()) };
        let record = LandmarkRecord {
            qid: 42,
            lon: 0,
            lat: 0,
            category: 2,
            language: *b"en",
            text_pages: 1,
            hours_ref: obcm::POI_HOURS_REF_NONE,
            osm: None,
            name,
            text,
            article,
            photo,
            photo_attribution,
        };
        section[SECTION_HEADER_LEN..SECTION_HEADER_LEN + RECORD_LEN].copy_from_slice(&record.encode());
        section[..4].copy_from_slice(&1u32.to_le_bytes());
        section[4..6].copy_from_slice(&(RECORD_LEN as u16).to_le_bytes());
        section[8..12].copy_from_slice(&((SECTION_HEADER_LEN + RECORD_LEN) as u32).to_le_bytes());
        let exact = section.len() as u32;
        section[12..16].copy_from_slice(&exact.to_le_bytes());
        section.resize(section.len().next_multiple_of(obcm_testkit::UNIT), 0);
        map[obcm::HEADER_LANDMARK_OFFSET_OFF..obcm::HEADER_LANDMARK_OFFSET_OFF + 4]
            .copy_from_slice(&obcm_testkit::scaled(start).to_le_bytes());
        map[obcm::HEADER_LANDMARK_LENGTH_OFF..obcm::HEADER_LANDMARK_LENGTH_OFF + 4]
            .copy_from_slice(&obcm_testkit::scaled(section.len()).to_le_bytes());
        map.extend_from_slice(&section);
        map
    }
    fn color(value: u16) -> Rgb888 {
        Rgb565::from(RawU16::new(value)).into()
    }
    fn draw(app: &mut App, reader: &Reader<'_>, frame: &mut RgbaFrame) {
        app.render_frame(None, frame, reader, None, 240.0, 320.0, color);
    }
    fn select(app: &mut App, reader: &Reader<'_>) {
        assert!(app.show_landmark_photo(
            Selection { qid: 42, map_generation: reader.generation(), record_index: 0 },
            "Authored photo"
        ));
    }
    fn assert_pixels(frame: &RgbaFrame) {
        use embedded_graphics::prelude::RgbColor;
        for n in 0..PHOTO_PIXELS {
            let pixel = (n % 64) as u8;
            let rgb =
                Rgb888::from(Rgb565::from(Rgb888::new((pixel >> 4) * 85, ((pixel >> 2) & 3) * 85, (pixel & 3) * 85)));
            let at = ((40 + n / PHOTO_WIDTH) * 240 + 12 + n % PHOTO_WIDTH) * 4;
            assert_eq!(&frame.as_rgba()[at..at + 3], &[rgb.r(), rgb.g(), rgb.b()]);
        }
    }

    #[test]
    fn retained_steps_fresh_capture_and_full_redraw_reconstruct_identical_pixels() {
        let bytes = map(true);
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&source, &tables, &cache);
        let mut app = App::new_idle(AppState::new(0, 0, 0.05));
        app.set_resident_frame(true);
        select(&mut app, &reader);
        let mut frame = RgbaFrame::new(240, 320);
        let mut phase = Preparer::default();
        draw(&mut app, &reader, &mut frame);
        phase.step(&mut app, Some(&reader), &mut frame, color);
        assert_eq!(app.photo_status(), Some(Status::Pending));
        // Losing the arena to another claimant restarts the same selection safely.
        phase = Preparer::default();
        phase.finish(&mut app, Some(&reader), &mut frame, color);
        assert_eq!(app.photo_status(), Some(Status::Complete));
        assert_pixels(&frame);
        let expected = frame.as_rgba().to_vec();
        draw(&mut app, &reader, &mut frame);
        assert_eq!(app.photo_status(), Some(Status::Fresh));
        phase.finish(&mut app, Some(&reader), &mut frame, color);
        assert_eq!(frame.as_rgba(), expected);
        app.set_resident_frame(false);
        let mut fresh = RgbaFrame::new(240, 320);
        draw(&mut app, &reader, &mut fresh);
        phase.finish(&mut app, Some(&reader), &mut fresh, color);
        assert_eq!(fresh.as_rgba(), expected);
    }

    #[test]
    fn drawer_blocks_writes_and_dismissal_replays_photo() {
        let bytes = map(true);
        let source = SliceSource(&bytes);
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&source, &tables, &cache);
        let mut app = App::new_idle(AppState::new(0, 0, 0.05));
        app.set_resident_frame(true);
        select(&mut app, &reader);
        let mut frame = RgbaFrame::new(240, 320);
        let mut phase = Preparer::default();
        draw(&mut app, &reader, &mut frame);
        phase.step(&mut app, Some(&reader), &mut frame, color);
        app.apply_chord(Chord::Quick);
        draw(&mut app, &reader, &mut frame);
        let covered = frame.as_rgba().to_vec();
        phase.step(&mut app, Some(&reader), &mut frame, color);
        assert_eq!(frame.as_rgba(), covered);
        assert!(!app.photo_pending());
        app.apply_gesture(Gesture::Back);
        draw(&mut app, &reader, &mut frame);
        phase.finish(&mut app, Some(&reader), &mut frame, color);
        assert_pixels(&frame);
        app.apply_gesture(Gesture::Back);
        draw(&mut app, &reader, &mut frame);
        let exited = frame.as_rgba().to_vec();
        phase.step(&mut app, Some(&reader), &mut frame, color);
        assert_eq!(frame.as_rgba(), exited);
        assert!(!app.photo_pending());
    }

    struct Fault {
        bytes: Vec<u8>,
        fail: Cell<bool>,
    }
    impl ByteSource for Fault {
        fn len(&self) -> u64 {
            self.bytes.len() as u64
        }
        fn read_at(&self, offset: u64, bytes: &mut [u8]) -> Result<(), Error> {
            if self.fail.get() {
                Err(Error::Io)
            } else {
                SliceSource(&self.bytes).read_at(offset, bytes)
            }
        }
    }
    #[test]
    fn failed_source_missing_photo_and_replaced_map_are_distinct_terminal_states() {
        let source = Fault { bytes: map(true), fail: Cell::new(false) };
        let tables = MapTables::parse(&source).unwrap();
        let cache = MapCache::new_boxed();
        let reader = Reader::new(&source, &tables, &cache);
        let mut app = App::new_idle(AppState::new(0, 0, 0.05));
        select(&mut app, &reader);
        let mut frame = RgbaFrame::new(240, 320);
        let mut phase = Preparer::default();
        draw(&mut app, &reader, &mut frame);
        phase.step(&mut app, Some(&reader), &mut frame, color);
        source.fail.set(true);
        phase.step(&mut app, Some(&reader), &mut frame, color);
        assert_eq!(app.photo_status(), Some(Status::Unavailable));
        assert!(!app.photo_pending());
        let background = color(obc_app::screen::palette::PARCHMENT);
        use embedded_graphics::prelude::RgbColor;
        let at = (40 * 240 + 12) * 4;
        assert_eq!(&frame.as_rgba()[at..at + 3], &[background.r(), background.g(), background.b()]);
        source.fail.set(false);
        draw(&mut app, &reader, &mut frame);
        phase.finish(&mut app, Some(&reader), &mut frame, color);
        assert_pixels(&frame);
        let missing = map(false);
        let replacement = SliceSource(&missing);
        let replacement_tables = MapTables::parse(&replacement).unwrap();
        let replacement_reader = Reader::new(&replacement, &replacement_tables, &cache);
        phase.step(&mut app, Some(&replacement_reader), &mut frame, color);
        assert_eq!(app.photo_status(), Some(Status::Unavailable));
        app.apply_gesture(Gesture::Back);
        select(&mut app, &replacement_reader);
        draw(&mut app, &replacement_reader, &mut frame);
        phase.finish(&mut app, Some(&replacement_reader), &mut frame, color);
        assert_eq!(app.photo_status(), Some(Status::Missing));
        assert!(!app.photo_pending());
    }
}
