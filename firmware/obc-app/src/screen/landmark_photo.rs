//! The selected photo's chrome. Pixel decoding runs in the mutable preparation phase.
use super::{palette::*, Ctx, RenderFrame, Transition};
use crate::{
    photo::{Selection, Status},
    Gesture, Msg,
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

    pub(crate) fn draw_status<D, F>(&self, target: &mut D, color: &F, language: crate::settings::Language)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let message = match self.status {
            Status::Missing => Msg::AssistantPhotoMissing,
            Status::Unavailable => Msg::AssistantPhotoUnavailable,
            _ => return,
        };
        Canvas::new(target, color).text(
            crate::i18n::t(message, language),
            Point::new(120, 142),
            Font::Label,
            TextAlign::Center,
            INK,
        );
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
                        return Transition::Push(super::Screen::PoiDetail(
                            super::PoiDetailScreen::new(poi).landmark(cx.landmarks.record.unwrap().category),
                        ));
                    }
                    return Transition::None;
                }
                _ => {}
            }
        }
        match gesture {
            Gesture::Back | Gesture::Press => Transition::Pop,
            _ => Transition::None,
        }
    }

    pub fn draw<D, F, S>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, S>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
        S: MapScene,
    {
        cv.clear(PARCHMENT);
        let page = self.linked.then_some(rx.landmarks.record).flatten().map(|record| {
            let total = record.text_pages as u16 + 1;
            (total, total)
        });
        super::landmarks::header(cv, self.title.as_str(), page);
        if self.linked {
            cv.round(obc_render::rect(4, 282, 232, 34), 6, AMBER);
            let label = match super::landmarks::visit_action(
                rx.landmarks,
                rx.poi_scratch,
                rx.place_local,
                rx.settings.bike_profile_idx,
            ) {
                Msg::AssistantVisit => Msg::AssistantPhotoVisit,
                Msg::AssistantClosed => Msg::AssistantPhotoClosed,
                Msg::AssistantNoAccess => Msg::AssistantPhotoNoAccess,
                _ => Msg::AssistantPhotoUnavailableHint,
            };
            cv.text(rx.t(label), Point::new(120, 286), Font::Label, TextAlign::Center, INK);
        }
    }
}
