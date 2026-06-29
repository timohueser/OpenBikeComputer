//! The Settings tree — the `Menu → Settings` family from the Claude Design mock
//! (`firmware/designs/Settings Screens.html`), in the same field-map style as the rest of the
//! UI. This module owns the **list** screen ([`SettingsScreen`]) and the reusable drawing
//! **kit** every settings screen shares (the toggle pill, the value/stepper field, the row
//! cursor); the individual screens live one file each ([`datetime`], [`units`], [`power`],
//! [`reset`]).
//!
//! **The two-level encoder model** (the thing the mock is really about):
//! - **Rotate** moves the amber row cursor between rows; while a field is open it changes that
//!   field's value instead.
//! - **Press** flips a toggle row, or *enters* a value row's stepper (a `▲▼` box marks the live
//!   field); pressing again steps field→field and off the end steps back out.
//! - **Back** steps out of an open field, else climbs one screen up.
//! - **Long-press** is reserved for the one guarded action, the factory [`reset`].
//!
//! Editing is **live**: a stepper writes straight into the shared [`Settings`](crate::Settings)
//! (so `Save & exit` is just a `Pop`, and stepping `back` out of a field is consistent with
//! that). [`App::apply_gesture`](crate::App::apply_gesture) notices the change with one `==` and
//! flags the host to persist it.

use embedded_graphics::{
    prelude::{DrawTarget, Point},
    primitives::Rectangle,
};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Canvas, RenderStats,
};

use crate::input::Gesture;

use super::{list_frame, palette, Ctx, Render, Screen, Transition, LIST_TOP};

mod datetime;
mod power;
mod reset;
mod units;

pub use datetime::DateTimeScreen;
pub use power::PowerScreen;
pub use reset::ResetScreen;
pub use units::UnitsScreen;

/// The Settings list entries, in order. `Units` is ours (the mock predates the metric/imperial
/// work); the other three are the mock's. Each row pushes its sub-screen.
const ITEMS: [&str; 4] = ["Date & Time", "Units", "Power", "Reset"];

/// Per-row height of the list — matches the main [`Menu`](super::MenuScreen) so the two read
/// identically.
const ROW_H: i32 = 52;

/// The Settings list — a nav menu whose rows open the individual settings screens. State is the
/// highlighted row; the same wrapping-list pattern as [`MenuScreen`](super::MenuScreen).
#[derive(Debug, Default)]
pub struct SettingsScreen {
    selected: usize,
}

impl SettingsScreen {
    pub fn new() -> Self {
        SettingsScreen { selected: 0 }
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            Gesture::Turn(n) => {
                self.selected = super::step_selection(self.selected, n, ITEMS.len());
                Transition::None
            }
            Gesture::Press => match self.selected {
                0 => Transition::Push(Screen::DateTime(DateTimeScreen::new())),
                1 => Transition::Push(Screen::Units(UnitsScreen::new())),
                2 => Transition::Push(Screen::Power(PowerScreen::new())),
                _ => Transition::Push(Screen::Reset(ResetScreen::new())),
            },
            Gesture::Back => Transition::Pop, // climb back to the main Menu
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let mut cv = Canvas::new(target, color_fn);

        list_frame(&mut cv, w, h, "SETTINGS", self.selected + 1, ITEMS.len());

        for (i, &name) in ITEMS.iter().enumerate() {
            let y = LIST_TOP + i as i32 * ROW_H;
            let mid = y + (ROW_H - 8) / 2;
            let selected = i == self.selected;
            if selected {
                cv.round(rect(16, y, w - 32, ROW_H - 8), 6, AMBER);
            }
            let bullet = if selected { INK } else { SUBTEXT };
            cv.triangle(Point::new(30, mid - 9), Point::new(30, mid + 9), Point::new(43, mid), bullet);
            cv.text(name, Point::new(54, mid - 14), Font::Body, TextAlign::Left, INK);
            if i + 1 < ITEMS.len() {
                cv.hline(20, y + ROW_H - 4, w - 40, RULE);
            }
        }
        RenderStats::default()
    }
}

// ---------------------------------------------------------------------------
// The shared kit — the reusable parts behind every settings screen (the mock's section 5).
// ---------------------------------------------------------------------------

/// Left inset of every settings row (clears the framed outline).
pub(super) const ROW_X: i32 = 14;

/// The full-width rectangle for the `i`-th row of height `h` starting at [`LIST_TOP`].
pub(super) fn row_rect(i: i32, y: i32, w: i32, h: i32) -> Rectangle {
    let _ = i;
    rect(ROW_X, y, w - 2 * ROW_X, h)
}

/// Paint a row's **row-focus** cursor: the amber bar behind a highlighted-but-not-editing row.
/// A no-op while editing (the live field's `▲▼` box is the cursor then) or when unselected — so
/// the two focus levels never both light up.
pub(super) fn row_cursor<D, F>(cv: &mut Canvas<D, F>, area: Rectangle, selected: bool, editing: bool)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    if selected && !editing {
        cv.round(area, 6, palette::AMBER);
    }
}

/// Draw a row's left-hand label (Body) with an optional muted sub-caption (Label) under it.
/// `area` is the row rect; returns nothing (the right-hand control is drawn by the caller).
pub(super) fn row_label<D, F>(cv: &mut Canvas<D, F>, area: Rectangle, label: &str, sub: Option<&str>)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let x = area.top_left.x + 10;
    match sub {
        Some(sub) => {
            cv.text(label, Point::new(x, area.top_left.y + 5), Font::Body, TextAlign::Left, palette::INK);
            cv.text(sub, Point::new(x, area.top_left.y + 30), Font::Label, TextAlign::Left, palette::SUBTEXT);
        }
        None => {
            // Single line: vertically centre the Body caps in the row.
            let y = area.top_left.y + (area.size.height as i32 - 22) / 2;
            cv.text(label, Point::new(x, y), Font::Body, TextAlign::Left, palette::INK);
        }
    }
}

/// Draw a **slider toggle** at the right of `area` — a classic switch whose white knob slides
/// left (off) / right (on), the rounded-rect track dark for off and green for on. No on/off
/// text: the knob position and track colour carry the state.
pub(super) fn toggle_slider<D, F>(cv: &mut Canvas<D, F>, area: Rectangle, on: bool)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let (tw, th) = (50, 28);
    let tx = area.top_left.x + area.size.width as i32 - tw - 4;
    let ty = area.top_left.y + (area.size.height as i32 - th) / 2;
    // Track: a rounded rectangle (small corners, not a pill).
    cv.round(rect(tx, ty, tw, th), 6, if on { palette::ON } else { palette::INK });
    // Knob: a white rounded square at the on/off end, with an even margin.
    let m = 4;
    let k = th - 2 * m;
    let kx = if on { tx + tw - m - k } else { tx + m };
    cv.round(rect(kx, ty + m, k, k), 4, palette::PARCHMENT);
}

/// Cap height (px) of each font tier — the vertical span the glyphs occupy, used to centre text
/// in a cell. Approximate but stable; tuned alongside the Terminus tiers in `obc-render`.
fn cap_height(font: Font) -> i32 {
    match font {
        Font::Label => 18,
        Font::Body => 22,
        Font::Display => 26,
    }
}

/// Draw a **stepper field** cell holding `text` in `font`. Inactive: just the text, **no
/// background**. Active (the live field): an amber fill plus an up-triangle above and a
/// down-triangle below (rotate to change it). `cell` must leave ~10 px of clearance above and
/// below for the arrows.
pub(super) fn stepper_field<D, F>(cv: &mut Canvas<D, F>, cell: Rectangle, text: &str, active: bool, font: Font)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let cx = cell.top_left.x + cell.size.width as i32 / 2;
    let ty = cell.top_left.y + (cell.size.height as i32 - cap_height(font)) / 2;
    if active {
        cv.round(cell, 4, palette::AMBER);
        let top = cell.top_left.y;
        let bot = cell.top_left.y + cell.size.height as i32;
        cv.triangle(Point::new(cx - 6, top - 3), Point::new(cx + 6, top - 3), Point::new(cx, top - 10), palette::INK);
        cv.triangle(Point::new(cx - 6, bot + 3), Point::new(cx + 6, bot + 3), Point::new(cx, bot + 10), palette::INK);
    }
    cv.text(text, Point::new(cx, ty), font, TextAlign::Center, palette::INK);
}

/// The shared `back tap = cancel / climb` footer hint, centred near the bottom — used by the
/// edit-flow screens so the single-tap exit is always discoverable.
pub(super) fn back_hint<D, F>(cv: &mut Canvas<D, F>, w: i32, h: i32, text: &str)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    cv.text(text, Point::new(w / 2, h - 26), Font::Label, TextAlign::Center, palette::SUBTEXT);
}
