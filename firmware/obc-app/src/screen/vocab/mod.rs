//! The shared **screen vocabulary** — the drawing primitives every screen composes its page from,
//! one module per concept:
//!
//! - [`chrome`] — the framed page header, the card glyphs, and the shared text/stroke helpers.
//! - [`list`] — the windowed scrolling-list widget and its wrapping cursor.
//! - [`rows`] — the settings row, the value picker, the stat-ledger row, the guarded option rows.
//! - [`tiles`] — the rounded stat panes of the riding grid and the Fields editor.
//!
//! Callers import from the owning module (`vocab::chrome::title_frame`), never through a re-export
//! at the [`screen`](crate::screen) root: which concept a helper belongs to is part of reading it.

pub(crate) mod chrome;
pub(crate) mod list;
pub(crate) mod rows;
pub(crate) mod tiles;
