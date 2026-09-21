//! The shared screen vocabulary: the drawing primitives every screen composes its page from, one
//! module per concept. Callers import from the owning module (`vocab::chrome::title_frame`), never
//! through a re-export at the [`screen`](crate::screen) root.

pub(crate) mod band;
pub(crate) mod card;
pub(crate) mod chrome;
pub(crate) mod fmt;
pub(crate) mod list;
pub(crate) mod marquee;
pub(crate) mod pager;
pub(crate) mod rows;
pub(crate) mod sheet;
pub(crate) mod spinner;
pub(crate) mod tiles;
