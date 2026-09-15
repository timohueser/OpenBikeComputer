//! The selected photo's chrome. Pixel decoding runs in the mutable preparation phase.
use super::{palette::*, Ctx, RenderFrame, Transition};
use crate::{
    photo::{Selection, Status},
    Gesture,
};
use embedded_graphics::{draw_target::DrawTarget, prelude::Point};
use obc_map_scene::MapScene;
use obc_render::{
    text::{Font, TextAlign},
    Canvas, Surface,
};

pub struct LandmarkPhotoScreen {
    pub(crate) selection: Selection,
    pub(crate) revision: u32,
    pub(crate) status: Status,
    pub(crate) covered_rebuild: bool,
    title: heapless::String<32>,
    pub(crate) linked: bool,
    pub(crate) source_valid: bool,
}

impl LandmarkPhotoScreen {
    pub fn new(selection: Selection, title: &str) -> Self {
        let mut label = heapless::String::new();
        for c in title.chars() {
            if label.push(c).is_err() {
                break;
            }
        }
        Self {
            selection,
            revision: 0,
            status: Status::Fresh,
            covered_rebuild: false,
            title: label,
            linked: false,
            source_valid: true,
        }
    }

    pub(crate) fn invalidate_source(&mut self) {
        self.source_valid = false;
        self.status = Status::Unavailable;
    }

    pub(crate) fn linked(mut self) -> Self {
        self.linked = true;
        self
    }

    pub(crate) fn invalidate(&mut self, covered: bool) {
        self.revision = self.revision.wrapping_add(1);
        self.status = Status::Fresh;
        self.covered_rebuild = covered;
    }

    pub(crate) fn draw_status<D, F>(&self, target: &mut D, color: &F)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let message = match self.status {
            Status::Missing => "No photo available",
            Status::Unavailable => "Photo unavailable",
            _ => return,
        };
        Canvas::new(target, color).text(message, Point::new(120, 142), Font::Label, TextAlign::Center, INK);
    }

    pub fn handle(&mut self, gesture: Gesture, cx: &mut Ctx) -> Transition {
        if self.linked {
            match gesture {
                Gesture::Step(n) => {
                    if let Some(record) = cx.landmarks.record {
                        cx.landmarks.page = if n < 0 { record.text_pages as u16 - 1 } else { 0 };
                    }
                    return Transition::Pop;
                }
                Gesture::Press => {
                    if let Some(poi) = super::landmarks::detail(cx.landmarks) {
                        return Transition::Push(super::Screen::PoiDetail(super::PoiDetailScreen::new(poi)));
                    }
                }
                _ => {}
            }
        }
        match gesture {
            Gesture::Back | Gesture::Press => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, _rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        cv.clear(PARCHMENT);
        cv.round(obc_render::rect(4, 4, 232, 34), 6, WOOD);
        cv.text(self.title.as_str(), Point::new(12, 9), Font::Label, TextAlign::Left, PARCHMENT);
        if self.linked {
            cv.round(obc_render::rect(4, 282, 232, 34), 6, AMBER);
            cv.text("Visit / Up or Down", Point::new(120, 286), Font::Label, TextAlign::Center, INK);
        }
    }
}
