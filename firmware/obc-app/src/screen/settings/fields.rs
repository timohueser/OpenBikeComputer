//! The Fields screen — choose which data fields the riding [`Statistics`](crate::screen) grid shows
//! and in what order. Reached from the [`Stats`](super::StatsScreen) screen's *Fields* row. Two idioms
//! on top of the shared two-level encoder model:
//!
//! - **Reordering.** *Press* grabs the highlighted field; rotating moves it, *press*/*back* drops it.
//!   While grabbed the row is anchored on screen (neighbours slide past it), so a two-span field
//!   hopping a whole row reads cleanly. A grabbed two-span field always begins a row —
//!   [`StatFieldList::move_item`](crate::stat_fields::StatFieldList::move_item) enforces it.
//! - **Removing.** A hold-to-delete footer (trash can + progress bar) erases the highlighted field —
//!   a deliberate gesture so a stray long-press can't drop a panel.
//!
//! The `Add field` row opens the [`AddField`](super::AddFieldScreen) picker. Editing is live into
//! [`Settings::stat_fields`](crate::Settings).

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
use crate::screen::{scrollbar, title_frame, window_start, Ctx, Render, Screen, Transition, LIST_TOP};

use super::AddFieldScreen;

/// Per-row height — a single Body label with room for the span badge / move arrows.
const ROW_H: i32 = 46;

/// Height of the hold-to-delete footer reserved at the bottom. Reserved whatever the cursor is on,
/// so the list doesn't reflow as you move between field and Add rows.
const FOOTER_H: i32 = 34;

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
                    self.selected = crate::screen::step_selection(self.selected, n, rows);
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

    /// First visible row, as a signed offset (can be negative). Normally [`window_start`]; while
    /// grabbed, pin the grabbed row to the middle slot for every position by scrolling the window
    /// virtually — so near the list ends the row stays centred with empty space above/below rather
    /// than drifting to the edge. The draw loop skips slots outside `0..rows`.
    fn window_first(&self, visible: usize, rows: usize) -> i32 {
        if self.grabbed {
            self.selected as i32 - (visible / 2) as i32
        } else {
            window_start(self.selected, visible, rows) as i32
        }
    }

    pub fn draw<D, F>(&self, target: &mut D, rx: &mut Render, color_fn: &F) -> RenderStats
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        use crate::screen::palette::*;
        let (w, h) = (rx.w as i32, rx.h as i32);
        let fields = rx.settings.stat_fields.as_slice();
        let len = fields.len();
        let add_row = len;
        let rows = len + 1;
        let mut cv = Canvas::new(target, color_fn);

        title_frame(&mut cv, w, h, "FIELDS", "");

        // Window the row list to what fits above the delete footer, scrolling to keep the cursor
        // visible (or anchoring the grabbed row).
        let list_h = h - LIST_TOP - 6 - FOOTER_H;
        let visible = (list_h / ROW_H).max(1) as usize;
        let first = self.window_first(visible, rows);

        for slot in 0..visible {
            // `first` is signed: while grabbed the window can scroll past either end, so some slots
            // map outside the list — those draw as empty space (the pinned row stays centred).
            let idx = first + slot as i32;
            if idx < 0 || idx as usize >= rows {
                continue;
            }
            let idx = idx as usize;
            let y = LIST_TOP + slot as i32 * ROW_H;
            let area = super::row_rect(0, y, w, ROW_H - 6);
            let selected = idx == self.selected;

            if idx == add_row {
                // Add-field row: a plus + label.
                super::row_cursor(&mut cv, area, selected, false);
                let midy = area.top_left.y + (area.size.height as i32 - 22) / 2;
                let px = area.top_left.x + 14;
                let pcy = midy + 11;
                cv.hline(px - 6, pcy, 13, INK);
                cv.vline(px, pcy - 6, 13, 1, INK);
                cv.text("Add field", Point::new(px + 18, midy), Font::Body, TextAlign::Left, INK);
            } else {
                let f = fields[idx];
                let grabbed = selected && self.grabbed;
                // A grabbed row gets the amber fill + move arrows; otherwise the plain row cursor
                // (suppressed while grabbed so they don't double up).
                super::row_cursor(&mut cv, area, selected, grabbed);
                if grabbed {
                    cv.round(area, 6, AMBER);
                    move_arrows(&mut cv, area);
                }
                super::row_label(&mut cv, area, f.name(), None);
                if !grabbed {
                    let badge_color = if selected { INK } else { SUBTEXT };
                    super::span_badge(&mut cv, area, f.span(), badge_color);
                }
            }
        }

        // The scrollbar wants the real (clamped) window position — the grabbed virtual offset can run
        // negative / past the end.
        let sb_first = first.clamp(0, rows.saturating_sub(visible) as i32) as usize;
        scrollbar(&mut cv, w - 8, LIST_TOP, visible as i32 * ROW_H, rows, sb_first, visible);
        delete_footer(&mut cv, w, h, self.selected < len, rx.hold_progress);
        RenderStats::default()
    }
}

/// Draw the hold-to-delete footer: a trash can + a warning-red progress bar filled by the live
/// encoder hold. Drawn only when a field row is highlighted (`on_field`); the Add row leaves it
/// blank. The delete itself fires from `handle`'s `Hold` arm.
fn delete_footer<D, F>(cv: &mut Canvas<D, F>, w: i32, h: i32, on_field: bool, hold: f32)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
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
fn draw_trash<D, F>(cv: &mut Canvas<D, F>, cx: i32, cy: i32, color: u16)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    let (bw, bh) = (11, 12);
    let (bx, by) = (cx - bw / 2, cy - bh / 2 + 1);
    cv.round_outline(rect(bx, by, bw, bh), 2, color); // can body
    cv.hline(bx - 2, by - 2, bw + 4, color); // lid
    cv.hline(cx - 2, by - 4, 5, color); // handle
    cv.vline(cx - 2, by + 3, bh - 5, 1, color); // ribs
    cv.vline(cx + 2, by + 3, bh - 5, 1, color);
}

/// Draw the up/down move arrows on a grabbed row's right edge — the "rotate to move me" cue.
fn move_arrows<D, F>(cv: &mut Canvas<D, F>, area: Rectangle)
where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
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
        let mut cx = Ctx { state: &mut st, activity: &mut act, settings: s, routes: &[], now_ms: 0 };
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

    /// While grabbed, the window pins the row to the middle slot for every position — including at
    /// the ends, where the window scrolls negative/past the end. Ungrabbed = plain scroll-to-reveal.
    #[test]
    fn grabbed_row_stays_pinned_mid_window() {
        let (visible, rows) = (5usize, 14usize);
        let plain = StatFieldsScreen { selected: 8, grabbed: false };
        assert_eq!(
            plain.window_first(visible, rows),
            window_start(8, visible, rows) as i32,
            "ungrabbed = plain scroll"
        );

        // For every selection — top, interior, bottom — the grabbed row sits at the mid slot.
        for sel in 0..rows {
            let s = StatFieldsScreen { selected: sel, grabbed: true };
            let slot = sel as i32 - s.window_first(visible, rows);
            assert_eq!(slot, (visible / 2) as i32, "the grabbed row stays pinned mid-window (sel={sel})");
        }
        // At the top the window offset really does go negative (empty space above the pinned row).
        let top = StatFieldsScreen { selected: 0, grabbed: true };
        assert!(top.window_first(visible, rows) < 0, "grabbing the first row scrolls the window past the top");
    }
}
