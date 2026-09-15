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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Fresh,
    Pending,
    Complete,
    Missing,
    Unavailable,
}

/// The only retained decode work; the board places it in its shared scratch arena.
pub struct Runtime {
    decoder: PhotoDecoder,
    selection: Option<Selection>,
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
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        if !matches!(page.status, Status::Fresh | Status::Pending) {
            self.cancel();
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
            if reader.generation() != page.selection.map_generation {
                return Err(());
            }
            let section = map_section(reader.source()).map_err(|_| ())?.ok_or(())?;
            let directory = LandmarkDirectory::read(&section).map_err(|_| ())?;
            let record = directory.record(&section, page.selection.record_index).map_err(|_| ())?;
            if record.qid != page.selection.qid {
                return Err(());
            }
            if record.photo.is_absent() {
                return if record.photo_attribution.is_absent() { Ok(Status::Missing) } else { Err(()) };
            }
            directory.content(&section, record.photo_attribution, MAX_ATTRIBUTION_BYTES).map_err(|_| ())?;
            let source = directory.content(&section, record.photo, PHOTO_MAX_COMPRESSED as u32).map_err(|_| ())?;
            self.decoder
                .step(&source, |offset, bytes| {
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
                })
                .map(|progress| match progress {
                    Progress::Pending => Status::Pending,
                    Progress::Complete => Status::Complete,
                })
                .map_err(|_| ())
        })();
        page.status = result.unwrap_or(Status::Unavailable);
        if matches!(page.status, Status::Missing | Status::Unavailable) {
            clear(target, &color);
        }
        if page.status != Status::Pending {
            self.cancel();
        }
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

    pub fn photo_pending(&self) -> bool {
        !self.overlay_active() && matches!(self.photo_status(), Some(Status::Fresh | Status::Pending))
    }

    /// Run after base drawing, before transient overlays and presentation. Continuation
    /// passes use the retained frame without redrawing the base. An ordinary base draw
    /// invalidates progress, including when a Sources page or drawer is dismissed.
    pub fn prepare_photo_step<D, F>(
        &mut self,
        runtime: &mut Runtime,
        reader: Option<&Reader<'_>>,
        target: &mut D,
        color: F,
    ) where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        if self.overlay_active() {
            runtime.cancel();
            return;
        }
        let Some(crate::screen::Screen::LandmarkPhoto(page)) = self.ui.stack.last_mut() else {
            runtime.cancel();
            return;
        };
        if reader.is_none_or(|reader| reader.generation() != page.selection.map_generation) {
            page.status = Status::Unavailable;
            clear(target, &color);
            runtime.cancel();
        }
        runtime.step(page, reader, target, &color);
        let message = match page.status {
            Status::Missing => Some("No photo available"),
            Status::Unavailable => Some("Photo unavailable"),
            _ => None,
        };
        if let Some(message) = message {
            use obc_render::{
                text::{Font, TextAlign},
                Surface,
            };
            let mut canvas = obc_render::Canvas::new(target, &color);
            canvas.text(message, Point::new(120, 142), Font::Label, TextAlign::Center, crate::screen::palette::INK);
        }
    }
}
