//! Incremental landmark photo preparation. No source or frame borrow survives a step.

use embedded_graphics::{
    draw_target::DrawTarget,
    pixelcolor::{raw::RawU16, Rgb565, Rgb888},
    prelude::*,
    primitives::Rectangle,
};
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
    fn valid(self, reader: &Reader<'_>) -> bool {
        match self {
            Self::Landmark(selection) => selection.map_generation == reader.generation(),
            Self::Peak(selection) => reader.with_peak_article(selection, |_, _, _| Ok(())).is_ok(),
        }
    }
    fn with_photo<T>(
        self,
        reader: &Reader<'_>,
        read: impl FnOnce(Option<&dyn obc_formats::io::ByteSource>) -> Result<T, ()>,
    ) -> Result<T, ()> {
        match self {
            Self::Peak(selection) => reader
                .with_peak_article(selection, |section, directory, record| {
                    if record.content[2].is_absent() {
                        return read(None).map_err(|_| obc_reader::Error::BadOffset);
                    }
                    directory.content(section, &record, 3, MAX_ATTRIBUTION_BYTES)?;
                    let source = directory.content(section, &record, 2, PHOTO_MAX_COMPRESSED as u32)?;
                    read(Some(&source)).map_err(|_| obc_reader::Error::BadOffset)
                })
                .map_err(|_| ()),
            Self::Landmark(selection) => {
                if reader.generation() != selection.map_generation {
                    return Err(());
                }
                let section = map_section(reader.source()).map_err(|_| ())?.ok_or(())?;
                let directory = LandmarkDirectory::read(&section).map_err(|_| ())?;
                let record = directory.record(&section, selection.record_index).map_err(|_| ())?;
                if record.qid != selection.qid {
                    return Err(());
                }
                if record.photo.is_absent() {
                    return if record.photo_attribution.is_absent() { read(None) } else { Err(()) };
                }
                directory.content(&section, record.photo_attribution, MAX_ATTRIBUTION_BYTES).map_err(|_| ())?;
                let source = directory.content(&section, record.photo, PHOTO_MAX_COMPRESSED as u32).map_err(|_| ())?;
                read(Some(&source))
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
    pub fn interactive(runtime: &'a mut Runtime, redraw: bool) -> Self {
        Self { runtime, steps: 1, redraw }
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
        }
    }

    pub fn cancel(&mut self) {
        self.selection = None;
    }

    pub(crate) fn step<D, F>(
        &mut self,
        page: &mut crate::screen::LandmarkPhotoScreen,
        reader: Option<&Reader<'_>>,
        target: &mut D,
        color: F,
        policy_enabled: Option<&core::cell::Cell<bool>>,
        language: crate::settings::Language,
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        if !page.source_valid || reader.is_none_or(|reader| !page.selection.valid(reader)) {
            page.status = Status::Unavailable;
            clear(target, &color);
        }
        if !matches!(page.status, Status::Fresh | Status::Pending) {
            self.cancel();
            if page.status == Status::Unavailable {
                clear(target, &color);
            }
            page.draw_status(target, &color, language);
            return;
        }
        if page.status == Status::Fresh || self.selection != Some(page.selection) || self.revision != page.revision {
            self.decoder.reset();
            self.selection = Some(page.selection);
            self.revision = page.revision;
            clear(target, &color);
        }
        let result = (|| {
            let reader = reader.ok_or(())?;
            page.selection.with_photo(reader, |source| {
                let Some(source) = source else {
                    return Ok(Status::Missing);
                };
                self.decoder
                    .step(source, |offset, bytes| {
                        if let Some(enabled) = policy_enabled {
                            enabled.set(false);
                        }
                        let _ = target.draw_iter(bytes.iter().enumerate().map(|(i, &pixel)| {
                            let n = offset + i;
                            let rgb = Rgb565::from(Rgb888::new(
                                ((pixel >> 4) & 3) * 85,
                                ((pixel >> 2) & 3) * 85,
                                (pixel & 3) * 85,
                            ));
                            Pixel(
                                Point::new(12 + (n % PHOTO_WIDTH) as i32, 40 + (n / PHOTO_WIDTH) as i32),
                                color(RawU16::from(rgb).into_inner()),
                            )
                        }));
                        if let Some(enabled) = policy_enabled {
                            enabled.set(true);
                        }
                    })
                    .map(|progress| match progress {
                        Progress::Pending => Status::Pending,
                        Progress::Complete => Status::Complete,
                    })
                    .map_err(|_| ())
            })
        })();
        page.status = result.unwrap_or(Status::Unavailable);
        if matches!(page.status, Status::Missing | Status::Unavailable) {
            clear(target, &color);
        }
        if page.status != Status::Pending {
            self.cancel();
        }
        page.draw_status(target, &color, language);
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
