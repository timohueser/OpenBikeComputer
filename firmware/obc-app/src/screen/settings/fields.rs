//! The Fields screen — choose which data fields the riding [`Statistics`](crate::screen) grid shows
//! and in what order, edited **as the grid itself**: the same 3×2 tile pages the Statistics view
//! draws (same [`page_fields`](stat_fields::page_fields) placement, same [`tile`](crate::screen)
//! renderer, live values), so what you arrange here is exactly what the ride shows. The cursor is
//! the amber tile; walking past a page's last tile flips to the next page (`page / pages` in the
//! title bar). Reached from the [`Stats`](super::StatsScreen) screen's *Fields* row. Two idioms on
//! top of the shared two-level encoder model:
//!
//! - **Reordering.** *Press* grabs the highlighted tile (move arrows appear); rotating moves it,
//!   *press*/*back* drops it. The grid reflows live per detent, so a two-span field's row-aligned
//!   hops ([`StatFieldList::move_item`](crate::stat_fields::StatFieldList::move_item)) are visible
//!   rather than inferred.
//! - **Removing.** A hold-to-delete footer (trash can + progress bar) erases the highlighted field —
//!   a deliberate gesture so a stray long-press can't drop a panel.
//!
//! The ghost `+` tile in the first free slot opens the [`AddField`](super::AddFieldScreen) picker —
//! a new field lands exactly where the ghost sits. Editing is live into
//! [`Settings::stat_fields`](crate::Settings).

use core::fmt::Write;

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::{list, title_frame, Ctx, Render, Screen, Transition, LIST_TOP};
use crate::stat_fields::{self, COLS, SLOTS_PER_PAGE};

use super::AddFieldScreen;

/// Height of the hold-to-delete footer reserved at the bottom, whatever the cursor is on, so the
/// grid doesn't reflow as you move between field tiles and the ghost Add tile.
const FOOTER_H: i32 = 34;

/// Gap between tiles — the Statistics grid's spacing, so the arrangement reads identically.
const GAP: i32 = 6;

/// Side margin of the grid (the Statistics chart margin, near enough that tiles look the same).
const GRID_X: i32 = 10;

/// The Fields screen. The row list is `[selected fields, in order] + [Add field…]`; `selected` is
/// the cursor over it, and `grabbed` lifts the selected field for moving.
#[derive(Debug, Default)]
pub struct StatFieldsScreen {
    selected: usize,
    grabbed: bool,
}

impl StatFieldsScreen {
    pub fn new() -> Self {
        StatFieldsScreen::default()
    }

    /// True while the cursor sits on a deletable field row (not the trailing Add row) — the
    /// hold-to-delete footer draws its fill then, so
    /// [`App::top_wants_hold_fill`](crate::App::top_wants_hold_fill) reports a charging hold as
    /// worth repainting here.
    pub(crate) fn selection_is_deletable(&self, settings: &crate::Settings) -> bool {
        self.selected < settings.stat_fields.len()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let len = cx.settings.stat_fields.len();
        let add_row = len; // rows: 0..len are the fields, `len` is the Add row
        let rows = len + 1;
        match g {
            Gesture::Turn(n) => {
                if self.grabbed && self.selected < len {
                    // Move the grabbed field one valid step per detent, following it with the cursor.
                    let mut idx = self.selected;
                    for _ in 0..n.unsigned_abs() {
                        idx = cx.settings.stat_fields.move_item(idx, n.signum());
                    }
                    self.selected = idx;
                } else {
                    return list::on_turn(&mut self.selected, n, rows);
                }
                Transition::None
            }
            Gesture::Press => {
                if self.selected == add_row {
                    return Transition::Push(Screen::AddField(AddFieldScreen::new()));
                }
                self.grabbed = !self.grabbed; // grab / drop the selected field
                Transition::None
            }
            // A completed hold deletes the highlighted field (the footer bar is the live feedback).
            // A stray hold on the Add row does nothing.
            Gesture::Hold => {
                if self.selected < len {
                    cx.settings.stat_fields.remove(self.selected);
                    self.grabbed = false;
                    // Keep the cursor in range — it may have been the last field; clamp to the Add row.
                    self.selected = self.selected.min(cx.settings.stat_fields.len());
                }
                Transition::None
            }
            // Back drops a grab first, else climbs to the Stats screen.
            Gesture::Back => {
                if self.grabbed {
                    self.grabbed = false;
                    Transition::None
                } else {
                    Transition::Pop
                }
            }
            Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use crate::screen::palette::*;
        let (w, h) = (rx.w, rx.h);
        let list = rx.settings.stat_fields; // `Copy` — frees `rx` for the readout borrow below
        let len = list.len();

        // The cursor's global slot decides the visible page; the ghost Add tile sits in the first
        // free slot, so browsing to it flips to its page too.
        let ghost_slot = stat_fields::next_free_slot(&list);
        let cur_slot = stat_fields::slot_of(&list, self.selected).unwrap_or(ghost_slot);
        let page = cur_slot / SLOTS_PER_PAGE;
        let pages = ghost_slot / SLOTS_PER_PAGE + 1;

        let mut counter: heapless::String<8> = heapless::String::new();
        if pages > 1 {
            let _ = write!(counter, "{} / {}", page + 1, pages);
        }
        title_frame(cv, w, h, "FIELDS", &counter);

        // Tile geometry: the Statistics grid's columns and gaps, with the rows stretched into the
        // space the chart occupies there — same arrangement, roomier panes.
        let grid_w = w - 2 * GRID_X;
        let col_w = (grid_w - GAP) / 2;
        let row_h = (h - FOOTER_H - LIST_TOP - 2 * GAP - 6) / stat_fields::ROWS_PER_PAGE as i32;
        let tile_rect = |slot: usize, span: u8| {
            let s = slot % SLOTS_PER_PAGE;
            let (col, row) = ((s % COLS) as i32, (s / COLS) as i32);
            let tw = if span == 2 { grid_w } else { col_w };
            rect(GRID_X + col * (col_w + GAP), LIST_TOP + row * (row_h + GAP), tw, row_h)
        };

        // The page's tiles — live cells through the same registry the Statistics grid draws with.
        let rdt = rx.readout();
        for (i, f) in list.as_slice().iter().enumerate() {
            let slot = stat_fields::slot_of(&list, i).unwrap_or(0);
            if slot / SLOTS_PER_PAGE == page {
                let area = tile_rect(slot, f.span());
                let is_sel = i == self.selected;
                let cell = f.cell(&rdt);
                let bg = if is_sel { AMBER } else { PARCHMENT_SHADE };
                crate::screen::tile(cv, area, &cell.caption, &cell.value, cell.arrow, cell.value_align, bg);
                if is_sel && self.grabbed {
                    move_arrows(cv, area);
                }
            }
        }

        // The ghost Add tile: tile anatomy (caption + a plus where the value goes) in outline form.
        if ghost_slot / SLOTS_PER_PAGE == page {
            let area = tile_rect(ghost_slot, 1);
            let is_sel = self.selected == len;
            if is_sel {
                cv.round(area, 5, AMBER);
            } else {
                cv.round_outline(area, 5, RULE);
                cv.round_outline(rect(area.top_left.x + 1, area.top_left.y + 1, col_w - 2, row_h - 2), 5, RULE);
            }
            let (x, y) = (area.top_left.x, area.top_left.y);
            cv.text("ADD", Point::new(x + 5, y + ((row_h - 48) / 2).max(4)), Font::Label, TextAlign::Left, SUBTEXT);
            let (px, py) = (x + col_w / 2, y + row_h / 2 + 8);
            cv.hline(px - 8, py, 17, INK);
            cv.vline(px, py - 8, 17, 2, INK);
        }

        delete_footer(cv, w, h, self.selected < len, rx.hold_progress);
    }
}

/// Draw the hold-to-delete footer: a trash can + a warning-red progress bar filled by the live
/// encoder hold. Drawn only when a field row is highlighted (`on_field`); the Add row leaves it
/// blank. The delete itself fires from `handle`'s `Hold` arm.
fn delete_footer(cv: &mut impl Surface, w: i32, h: i32, on_field: bool, hold: f32) {
    use crate::screen::palette::*;
    let fy = h - FOOTER_H;
    cv.hline(super::ROW_X, fy, w - 2 * super::ROW_X, RULE);
    if !on_field {
        return;
    }
    let p = hold.clamp(0.0, 1.0);
    let midy = fy + FOOTER_H / 2;
    draw_trash(cv, super::ROW_X + 16, midy, WARNING);
    let bh = 12;
    let (bx, by) = (super::ROW_X + 36, midy - bh / 2);
    let bw = w - super::ROW_X - 4 - bx;
    cv.round(rect(bx, by, bw, bh), 6, PARCHMENT_SHADE);
    let fill = (bw as f32 * p) as i32;
    if fill > 0 {
        cv.round(rect(bx, by, fill, bh), 6, WARNING);
    }
}

/// Draw a small trash-can glyph centred at `(cx, cy)`: a lidded can with a handle and ribs.
fn draw_trash(cv: &mut impl Surface, cx: i32, cy: i32, color: u16) {
    let (bw, bh) = (11, 12);
    let (bx, by) = (cx - bw / 2, cy - bh / 2 + 1);
    cv.round_outline(rect(bx, by, bw, bh), 2, color); // can body
    cv.hline(bx - 2, by - 2, bw + 4, color); // lid
    cv.hline(cx - 2, by - 4, 5, color); // handle
    cv.vline(cx - 2, by + 3, bh - 5, 1, color); // ribs
    cv.vline(cx + 2, by + 3, bh - 5, 1, color);
}

/// Draw the up/down move arrows on a grabbed row's right edge — the "rotate to move me" cue.
fn move_arrows(cv: &mut impl Surface, area: Rectangle) {
    use crate::screen::palette::INK;
    let x = area.top_left.x + area.size.width as i32 - 16;
    let midy = area.top_left.y + area.size.height as i32 / 2;
    cv.triangle(Point::new(x - 7, midy - 3), Point::new(x + 7, midy - 3), Point::new(x, midy - 12), INK);
    cv.triangle(Point::new(x - 7, midy + 3), Point::new(x + 7, midy + 3), Point::new(x, midy + 12), INK);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut StatFieldsScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let scratch = crate::screen::PoiScratch::new();
        let mut cx = Ctx {
            state: &mut st,
            activity: &mut act,
            settings: s,
            routes: &[],
            rides: &[],
            nav_profiles: &crate::NavProfiles::EMPTY,
            poi_scratch: &scratch,
            now_ms: 0,
        };
        scr.handle(g, &mut cx)
    }

    /// Grab the first field, move it down with a turn, and drop it — the order changes and the cursor
    /// follows the grabbed field.
    #[test]
    fn grab_move_drop_reorders() {
        let mut s = Settings::default(); // six default fields, cursor starts on the first
        let mut scr = StatFieldsScreen::new();
        run(&mut scr, &mut s, Gesture::Press); // grab field 0
        assert!(scr.grabbed);
        let first_before = s.stat_fields.as_slice()[0];
        run(&mut scr, &mut s, Gesture::Turn(1)); // move it down one
        assert_eq!(scr.selected, 1, "the cursor follows the grabbed field");
        assert_eq!(s.stat_fields.as_slice()[1], first_before, "the field moved down a slot");
        run(&mut scr, &mut s, Gesture::Press); // drop
        assert!(!scr.grabbed);
    }

    /// A completed hold deletes the highlighted field; the selection shrinks and the cursor stays in
    /// range. (The footer bar is just the live feedback for the hold.)
    #[test]
    fn hold_deletes_the_highlighted_field() {
        let mut s = Settings::default();
        let before = s.stat_fields.len();
        let removed = s.stat_fields.as_slice()[0];
        let mut scr = StatFieldsScreen::new();
        run(&mut scr, &mut s, Gesture::Hold);
        assert_eq!(s.stat_fields.len(), before - 1, "the field is removed");
        assert_ne!(s.stat_fields.as_slice()[0], removed, "and it was the highlighted one");
    }

    /// Deleting the last field clamps the cursor onto the Add row rather than off the end.
    #[test]
    fn deleting_the_last_field_lands_on_the_add_row() {
        let mut s = Settings::default();
        let mut scr = StatFieldsScreen::new();
        let len = s.stat_fields.len();
        // Walk to the last field (index len-1).
        for _ in 0..len - 1 {
            run(&mut scr, &mut s, Gesture::Turn(1));
        }
        assert_eq!(scr.selected, len - 1);
        run(&mut scr, &mut s, Gesture::Hold); // delete it
        assert_eq!(scr.selected, s.stat_fields.len(), "cursor clamps to the Add row");
    }

    /// Press on the Add row pushes the picker; Back from the bare list climbs to the Stats screen.
    #[test]
    fn add_row_opens_picker_and_back_pops() {
        let mut s = Settings::default();
        let len = s.stat_fields.len();
        let mut scr = StatFieldsScreen::new();
        run(&mut scr, &mut s, Gesture::Turn(len as i32)); // cursor → Add row (index len)
        assert_eq!(scr.selected, len);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Push(Screen::AddField(_))));
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }

    /// Back drops a grab before it pops — the staged escape.
    #[test]
    fn back_drops_a_grab_first() {
        let mut s = Settings::default();
        let mut scr = StatFieldsScreen::new();
        run(&mut scr, &mut s, Gesture::Press); // grab field 0
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.grabbed, "back dropped the grab, didn't pop");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "a second back pops");
    }

    /// The editor's cursor→page mapping: with seven fields (two pages), walking the cursor past
    /// the sixth tile lands on page 2 — and the ghost Add tile (index == len) lives on the page
    /// after the last field's.
    #[test]
    fn cursor_page_follows_the_placement_walk() {
        let mut s = Settings::default(); // six single-span fields — page 1 exactly full
        s.stat_fields.push(crate::stat_fields::StatField::Clock);
        let list = s.stat_fields;
        // Fields 0..=5 sit on page 0, the clock (index 6) on page 1.
        for i in 0..6 {
            assert_eq!(stat_fields::slot_of(&list, i).unwrap() / SLOTS_PER_PAGE, 0, "field {i} is on page 1");
        }
        assert_eq!(stat_fields::slot_of(&list, 6).unwrap() / SLOTS_PER_PAGE, 1, "the clock starts page 2");
        // The ghost Add tile follows the clock on page 2 (the 2-span clock consumed slots 6..8).
        assert_eq!(stat_fields::next_free_slot(&list) / SLOTS_PER_PAGE, 1, "the Add ghost shares page 2");
        assert_eq!(stat_fields::slot_of(&list, 7), None, "past the selection there is no slot");
    }
}
