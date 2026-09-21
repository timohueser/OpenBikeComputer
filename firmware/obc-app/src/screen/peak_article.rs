//! Peak-only entry to the common article presentation.
use super::{Ctx, RenderFrame, Screen, Transition};
use crate::Gesture;
use embedded_graphics::draw_target::DrawTarget;
use obc_render::Canvas;

#[derive(Debug)]
pub struct PeakArticleScreen {
    pub(crate) selection: obc_reader::peaks::Selection,
    pub(crate) source: obc_formats::obcm::SourceId,
}
impl PeakArticleScreen {
    pub fn handle(&mut self, gesture: Gesture, cx: &mut Ctx) -> Transition {
        match gesture {
            Gesture::Back | Gesture::Press => Transition::Pop,
            Gesture::Step(n) => {
                let state = &mut cx.landmarks;
                let Some(article) = state.article.filter(|_| state.ready()) else { return Transition::None };
                let next = super::vocab::list::step_selection(
                    state.page as usize,
                    n,
                    article.text_pages as usize + usize::from(state.photo_available),
                );
                if next == article.text_pages as usize {
                    if let Some(selection) = state.selection() {
                        return Transition::Push(Screen::LandmarkPhoto(
                            super::LandmarkPhotoScreen::content(selection, &state.name).linked(),
                        ));
                    }
                } else {
                    state.page = next as u16;
                }
                Transition::None
            }
            _ => Transition::None,
        }
    }
    pub fn draw<D, F>(&self, cv: &mut Canvas<D, F>, rx: &mut RenderFrame<'_, '_>)
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        super::landmarks::reading(cv, rx, false);
    }
}
