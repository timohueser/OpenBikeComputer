//! A host-only recorder of the text a frame asks for: every [`Surface::text`](crate::Surface::text)
//! call on a [`Canvas`](crate::Canvas), as the box it occupies, its face and its string.
//!
//! A recording `DrawTarget` sees loose lit pixels, never a string, so a copy-fit gate cannot be
//! built above the render path. This tap is that seam. It records what the screen asks to draw,
//! before the clip rejects anything, so a clipped repaint reports the same text a full frame does.

extern crate std;

use std::{cell::RefCell, string::String, vec::Vec};

use embedded_graphics::primitives::Rectangle;

use crate::text::Font;

/// One recorded text draw.
#[derive(Clone, Debug)]
pub struct TextDraw {
    /// The glyph-cell box, in panel coordinates.
    pub area: Rectangle,
    pub font: Font,
    pub text: String,
}

std::thread_local! {
    /// `Some` only while [`record`] runs, so an ordinary frame costs one thread-local read.
    static LOG: RefCell<Option<Vec<TextDraw>>> = const { RefCell::new(None) };
}

/// Run `f` and return every text draw it made. One recording per thread at a time: a nested call
/// discards what the outer one collected so far.
pub fn record(f: impl FnOnce()) -> Vec<TextDraw> {
    LOG.with(|log| *log.borrow_mut() = Some(Vec::new()));
    f();
    LOG.with(|log| log.borrow_mut().take()).unwrap_or_default()
}

pub(crate) fn note(area: Rectangle, font: Font, text: &str) {
    LOG.with(|log| {
        if let Some(draws) = log.borrow_mut().as_mut() {
            draws.push(TextDraw { area, font, text: String::from(text) });
        }
    });
}
