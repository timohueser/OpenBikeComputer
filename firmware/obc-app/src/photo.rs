//! Incremental landmark photo preparation. No source or frame borrow survives a step.

use core::ops::Range;
use embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::{raw::RawU16, Rgb565, Rgb888},
    prelude::*,
    primitives::Rectangle,
};
use obc_formats::io::{ByteSource, WindowSource};
use obc_formats::obcm::landmarks::{MAX_ATTRIBUTION_BYTES, PHOTO_HEIGHT, PHOTO_MAX_COMPRESSED, PHOTO_WIDTH};
use obc_reader::{
    landmarks::{map_section, LandmarkDirectory},
    photo::{PhotoDecoder, Progress},
    Reader,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub qid: u64,
    pub map_generation: u32,
    pub record_index: u32,
}

/// Distinct public lookup paths share only the photo decoder.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ContentSelection {
    Landmark(Selection),
    Peak(obc_reader::peaks::Selection),
}
impl ContentSelection {
    fn generation(self) -> u32 {
        match self {
            Self::Landmark(selection) => selection.map_generation,
            Self::Peak(selection) => selection.generation(),
        }
    }
    /// The photo's bytes in the map source, or `None` for a record without a photo.
    fn photo(self, reader: &Reader<'_>) -> Result<Option<Range<u64>>, ()> {
        let absolute = |section: &WindowSource<'_>, photo: WindowSource<'_>| {
            let start = section.offset() + photo.offset();
            start..start + photo.len()
        };
        match self {
            Self::Peak(selection) => reader
                .with_peak_article(selection, |section, directory, record| {
                    if record.content[2].is_absent() {
                        return Ok(None);
                    }
                    directory.content(section, &record, 3, MAX_ATTRIBUTION_BYTES)?;
                    let photo = directory.content(section, &record, 2, PHOTO_MAX_COMPRESSED as u32)?;
                    Ok(Some(absolute(section, photo)))
                })
                .map_err(|_| ()),
            Self::Landmark(selection) => {
                let section = map_section(reader.source()).map_err(|_| ())?.ok_or(())?;
                let directory = LandmarkDirectory::read(&section).map_err(|_| ())?;
                let record = directory.record(&section, selection.record_index).map_err(|_| ())?;
                if record.qid != selection.qid {
                    return Err(());
                }
                if record.photo.is_absent() {
                    return if record.photo_attribution.is_absent() { Ok(None) } else { Err(()) };
                }
                directory.content(&section, record.photo_attribution, MAX_ATTRIBUTION_BYTES).map_err(|_| ())?;
                let photo = directory.content(&section, record.photo, PHOTO_MAX_COMPRESSED as u32).map_err(|_| ())?;
                Ok(Some(absolute(&section, photo)))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Fresh,
    Pending,
    Complete,
    Missing,
    Unavailable,
}

/// Photo work within one base-before-overlay render pass.
pub struct FramePhoto<'a> {
    pub(crate) runtime: &'a mut Runtime,
    pub(crate) steps: usize,
    pub(crate) redraw: bool,
}
impl<'a> FramePhoto<'a> {
    /// Sixteen steps read at most 4 KiB of the photo, so a pass stays short and a photo still
    /// completes in a few passes.
    pub fn interactive(runtime: &'a mut Runtime, redraw: bool) -> Self {
        Self { runtime, steps: 16, redraw }
    }
    pub fn capture(runtime: &'a mut Runtime) -> Self {
        Self { runtime, steps: PHOTO_MAX_COMPRESSED + obc_formats::obcm::landmarks::PHOTO_PIXELS + 1, redraw: true }
    }
}

/// The only retained decode work; the board places it in its shared scratch arena.
pub struct Runtime {
    decoder: PhotoDecoder,
    selection: Option<ContentSelection>,
    revision: u32,
    /// Resolved once per decode, so a step reads only photo bytes.
    photo: Range<u64>,
}

impl Default for Runtime {
    fn default() -> Self {
        Self::new()
    }
}
impl Runtime {
    crate::placement::define_placement_constructors! {
        pub fn new();
        pub unsafe fn init_in_place;
        fields {
            decoder: PhotoDecoder::new() => PhotoDecoder::init_in_place,
            selection: None,
            revision: 0,
            photo: 0..0,
        }
    }

    pub fn cancel(&mut self) {
        self.selection = None;
    }

    /// Run up to `steps` decoder steps for `page`, then draw its status.
    pub(crate) fn step<D, F>(
        &mut self,
        page: &mut crate::screen::LandmarkPhotoScreen,
        reader: Option<&Reader<'_>>,
        target: &mut D,
        color: F,
        language: crate::settings::Language,
        steps: usize,
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let reader = reader.filter(|reader| page.source_valid && page.selection.generation() == reader.generation());
        page.status = match reader {
            None => Status::Unavailable,
            Some(reader) if matches!(page.status, Status::Fresh | Status::Pending) => {
                self.decode(page, reader, target, &color, steps).unwrap_or(Status::Unavailable)
            }
            Some(_) => page.status,
        };
        if matches!(page.status, Status::Missing | Status::Unavailable) {
            clear(target, &color);
        }
        if page.status != Status::Pending {
            self.cancel();
        }
        page.draw_status(target, &color, language);
    }

    fn decode<D, F>(
        &mut self,
        page: &crate::screen::LandmarkPhotoScreen,
        reader: &Reader<'_>,
        target: &mut D,
        color: &F,
        steps: usize,
    ) -> Result<Status, ()>
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        if page.status == Status::Fresh || self.selection != Some(page.selection) || self.revision != page.revision {
            self.decoder.reset();
            self.selection = Some(page.selection);
            self.revision = page.revision;
            clear(target, color);
            match page.selection.photo(reader)? {
                Some(photo) => self.photo = photo,
                None => return Ok(Status::Missing),
            }
        }
        let source =
            WindowSource::new(reader.source(), self.photo.start, self.photo.end - self.photo.start).ok_or(())?;
        let palette: [D::Color; 64] = core::array::from_fn(|pixel| {
            let level = |shift: usize| ((pixel >> shift) & 3) as u8 * 85;
            color(RawU16::from(Rgb565::from(Rgb888::new(level(4), level(2), level(0)))).into_inner())
        });
        for _ in 0..steps {
            let progress = self
                .decoder
                .step(&source, |offset, bytes| {
                    let _ = target.draw_iter(bytes.iter().enumerate().map(|(i, &pixel)| {
                        let n = offset + i;
                        Pixel(
                            Point::new(12 + (n % PHOTO_WIDTH) as i32, 40 + (n / PHOTO_WIDTH) as i32),
                            palette[usize::from(pixel)],
                        )
                    }));
                })
                .map_err(|_| ())?;
            if progress == Progress::Complete {
                return Ok(Status::Complete);
            }
        }
        Ok(Status::Pending)
    }
}

pub fn rectangle() -> Rectangle {
    Rectangle::new(Point::new(12, 40), Size::new(PHOTO_WIDTH as u32, PHOTO_HEIGHT as u32))
}
fn clear<D: DrawTarget>(target: &mut D, color: &impl Fn(u16) -> D::Color) {
    let _ = target.fill_solid(&rectangle(), color(crate::screen::palette::PARCHMENT));
}

impl crate::App {
    /// Enter a photo selected from the current map's landmark directory.
    pub fn show_landmark_photo(&mut self, selection: Selection, title: &str) -> bool {
        if self.ui.stack.len() == crate::screen::MAX_DEPTH {
            return false;
        }
        crate::screen::apply(
            &mut self.ui.stack,
            crate::screen::Transition::Push(crate::screen::Screen::LandmarkPhoto(
                crate::screen::LandmarkPhotoScreen::new(selection, title),
            )),
        );
        self.ui.map_dirty = true;
        true
    }

    pub fn photo_status(&self) -> Option<Status> {
        match self.ui.stack.last() {
            Some(crate::screen::Screen::LandmarkPhoto(page)) => Some(page.status),
            _ => None,
        }
    }

    pub fn photo_base_active(&self) -> bool {
        matches!(crate::screen::base_screen(&self.ui.stack), Some(crate::screen::Screen::LandmarkPhoto(_)))
    }

    pub fn photo_pending(&self) -> bool {
        if self.overlay_active() {
            return false;
        }
        let base = crate::screen::base_screen(&self.ui.stack);
        matches!(base, Some(crate::screen::Screen::LandmarkPhoto(page))
            if (self.photo_status().is_some() || page.covered_rebuild)
            && matches!(page.status, Status::Fresh | Status::Pending))
    }
}
