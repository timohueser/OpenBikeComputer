//! The shared **screen vocabulary** — the drawing primitives every screen composes its page from,
//! one module per concept:
//!
//! - [`band`] — the elevation silhouette, its connected top stroke, and the peak label.
//! - [`card`] — selection, guarded activation, dismissal, and drawing for card action rows.
//! - [`chrome`] — the framed page header, the card glyphs, and the shared text/stroke helpers.
//! - [`fmt`] — every quantity readout: distance, speed, elevation, duration, date, temperature.
//! - [`list`] — the windowed scrolling-list widget and its wrapping cursor.
//! - [`pager`] — the two-page auto-flip the detail compositions share.
//! - [`rows`] — the settings row, the value picker, the stat-ledger row, the guarded option rows.
//! - [`sheet`] — the two drawers' shared arrival/page curves and the committed-value tick.
//! - [`spinner`] — the free-spinning wait needle and the dirty disc it repaints inside.
//! - [`tiles`] — the rounded stat panes of the riding grid and the Fields editor.
//!
//! Callers import from the owning module (`vocab::chrome::title_frame`), never through a re-export
//! at the [`screen`](crate::screen) root: which concept a helper belongs to is part of reading it.

pub(crate) mod band;
pub(crate) mod card;
pub(crate) mod chrome;
pub(crate) mod fmt;
pub(crate) mod list;
pub(crate) mod pager;
pub(crate) mod rows;
pub(crate) mod sheet;
pub(crate) mod spinner;
pub(crate) mod tiles;
