//! The Fields screen: which data fields the Statistics grid shows, and in what order. It draws the
//! same tiles as the Statistics view, so the arrangement is what the ride shows. Press grabs the
//! highlighted tile for moving, a completed hold deletes it, and the ghost `+` tile opens the picker.

use core::fmt::Write;

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::screen::vocab::chrome::{title_frame, LIST_TOP};
use crate::screen::vocab::list;
use crate::screen::vocab::rows::ROW_X;
use crate::screen::vocab::tiles::{category_tile, tile, waypoint_panel_ghost};
use crate::screen::{Ctx, Render, Screen, Transition};
use crate::stat_fields::{self, COLS, SLOTS_PER_PAGE};
use crate::Msg;

use super::AddFieldScreen;

/// Height of the footer. It is reserved for every cursor position, so the grid does not reflow.
const FOOTER_H: i32 = 34;

/// Gap between tiles. It is the Statistics grid spacing, so the arrangement reads the same.
const GAP: i32 = 6;

/// Side margin of the grid. It is the Statistics chart margin.
const GRID_X: i32 = 10;

/// The rows are the selected fields in order, then a trailing Add row.
#[derive(Debug, Default)]
pub struct StatFieldsScreen {
    selected: usize,
    grabbed: bool,
}

impl StatFieldsScreen {
    pub fn new() -> Self {
        StatFieldsScreen::default()
    }

    /// True while the cursor sits on a field row, not on the trailing Add row.
    pub(crate) fn selection_is_deletable(&self, settings: &crate::Settings) -> bool {
        self.selected < settings.stat_fields.len()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let len = cx.settings.stat_fields.len();
        let add_row = len; // rows: 0..len are the fields, `len` is the Add row
        let rows = len + 1;
        match g {
            Gesture::Step(n) => {
                if self.grabbed && self.selected < len {
                    let mut idx = self.selected;
                    for _ in 0..n.unsigned_abs() {
                        idx = cx.settings.stat_fields.move_item(idx, n.signum());
                    }
                    self.selected = idx;
                } else {
                    return list::on_step(&mut self.selected, n, rows);
                }
                Transition::None
            }
            Gesture::Press => {
                if self.selected == add_row {
                    return Transition::Push(Screen::AddField(AddFieldScreen::new()));
                }
                self.grabbed = !self.grabbed;
                Transition::None
            }
            Gesture::Hold => {
                if self.selected < len {
                    cx.settings.stat_fields.remove(self.selected);
                    self.grabbed = false;
                    self.selected = self.selected.min(cx.settings.stat_fields.len());
                }
                Transition::None
            }
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

        // The cursor slot decides the visible page. The ghost Add tile sits in the first free slot.
        let ghost_slot = stat_fields::next_free_slot(&list);
        let cur_slot = stat_fields::slot_of(&list, self.selected).unwrap_or(ghost_slot);
        let page = cur_slot / SLOTS_PER_PAGE;
        let pages = ghost_slot / SLOTS_PER_PAGE + 1;

        let mut counter: heapless::String<8> = heapless::String::new();
        if pages > 1 {
            let _ = write!(counter, "{} / {}", page + 1, pages);
        }
        title_frame(cv, w, h, rx.t(Msg::FieldsTitle), &counter);

        // Tile geometry: the Statistics columns and gaps, with the rows stretched into the chart space.
        let grid_w = w - 2 * GRID_X;
        let col_w = (grid_w - GAP) / 2;
        let row_h = (h - FOOTER_H - LIST_TOP - 2 * GAP - 6) / stat_fields::ROWS_PER_PAGE as i32;
        // A two-span field fills the grid width, and a multi-row panel the full grid.
        let tile_rect = |slot: usize, span: u8, rows: u8| {
            let s = slot % SLOTS_PER_PAGE;
            let (col, row) = ((s % COLS) as i32, (s / COLS) as i32);
            let tw = if span == 2 { grid_w } else { col_w };
            let th = if rows > 1 {
                row_h * stat_fields::ROWS_PER_PAGE as i32 + GAP * (stat_fields::ROWS_PER_PAGE as i32 - 1)
            } else {
                row_h
            };
            rect(GRID_X + col * (col_w + GAP), LIST_TOP + row * (row_h + GAP), tw, th)
        };

        let rdt = rx.readout();
        for (i, f) in list.as_slice().iter().enumerate() {
            let slot = stat_fields::slot_of(&list, i).unwrap_or(0);
            if slot / SLOTS_PER_PAGE == page {
                let area = tile_rect(slot, f.span(), f.rows());
                let is_sel = i == self.selected;
                let bg = if is_sel { AMBER } else { PARCHMENT_SHADE };
                let caption_color = if is_sel { SUBTEXT_ON_ACCENT } else { SUBTEXT };
                let value_color = if is_sel { ON_ACCENT } else { SUBTEXT };
                if f.rows() > 1 {
                    waypoint_panel_ghost(cv, area, rdt.language, bg, caption_color);
                } else {
                    let mut cell = f.cell(&rdt);
                    ghost_value(*f, &mut cell, rdt.language);
                    match f.category() {
                        Some(cat) => {
                            category_tile(cv, area, cat, &cell.caption, &cell.value, bg, caption_color, value_color)
                        }
                        None => tile(
                            cv,
                            area,
                            &rx.marquee,
                            &cell.caption,
                            &cell.value,
                            cell.arrow,
                            cell.value_align,
                            bg,
                            caption_color,
                            value_color,
                        ),
                    }
                }
                if is_sel && self.grabbed {
                    move_arrows(cv, area);
                }
            }
        }

        // The ghost Add tile: a caption and a plus where the value goes.
        if ghost_slot / SLOTS_PER_PAGE == page {
            let area = tile_rect(ghost_slot, 1, 1);
            let is_sel = self.selected == len;
            if is_sel {
                cv.round(area, 5, AMBER);
            } else {
                cv.round_outline(area, 5, RULE);
                cv.round_outline(rect(area.top_left.x + 1, area.top_left.y + 1, col_w - 2, row_h - 2), 5, RULE);
            }
            let (x, y) = (area.top_left.x, area.top_left.y);
            cv.text(
                rx.t(Msg::FieldsAdd),
                Point::new(x + 5, y + ((row_h - 48) / 2).max(4)),
                Font::Label,
                TextAlign::Left,
                if is_sel { SUBTEXT_ON_ACCENT } else { SUBTEXT },
            );
            let (px, py) = (x + col_w / 2, y + row_h / 2 + 8);
            let ink = if is_sel { ON_ACCENT } else { INK };
            cv.hline(px - 8, py, 17, ink);
            cv.vline(px, py - 8, 17, 2, ink);
        }

        delete_footer(cv, w, h, self.selected < len, rx.hold_progress);
    }
}

/// Draw the footer: a trash can and a progress bar that the live hold fills. The Add row leaves it blank.
fn delete_footer(cv: &mut impl Surface, w: i32, h: i32, on_field: bool, hold: f32) {
    use crate::screen::palette::*;
    let fy = h - FOOTER_H;
    cv.hline(ROW_X, fy, w - 2 * ROW_X, RULE);
    if !on_field {
        return;
    }
    let p = hold.clamp(0.0, 1.0);
    let midy = fy + FOOTER_H / 2;
    draw_trash(cv, ROW_X + 16, midy, WARNING);
    let bh = 12;
    let (bx, by) = (ROW_X + 36, midy - bh / 2);
    let bw = w - ROW_X - 4 - bx;
    cv.round(rect(bx, by, bw, bh), 6, PARCHMENT_SHADE);
    let fill = (bw as f32 * p) as i32;
    if fill > 0 {
        cv.round(rect(bx, by, fill, bh), 6, WARNING);
    }
}

fn draw_trash(cv: &mut impl Surface, cx: i32, cy: i32, color: u16) {
    let (bw, bh) = (11, 12);
    let (bx, by) = (cx - bw / 2, cy - bh / 2 + 1);
    cv.round_outline(rect(bx, by, bw, bh), 2, color); // can body
    cv.hline(bx - 2, by - 2, bw + 4, color); // lid
    cv.hline(cx - 2, by - 4, 5, color); // handle
    cv.vline(cx - 2, by + 3, bh - 5, 1, color); // ribs
    cv.vline(cx + 2, by + 3, bh - 5, 1, color);
}

/// Replace the cell value with a fixed sample. The editor has no route and no fix, so a sample keeps
/// the tiles realistic. Category tiles also get the localized category name as their caption, so the
/// editor shows the same picture on every device.
fn ghost_value(
    field: crate::stat_fields::StatField,
    cell: &mut crate::stat_fields::StatCell,
    lang: crate::settings::Language,
) {
    use crate::stat_fields::StatField as F;
    use obc_reader::PoiCategory;
    if let Some(cat) = field.category() {
        cell.caption.clear();
        let _ = cell.caption.push_str(field.name(lang));
        cell.value.clear();
        let _ = cell.value.push_str(match cat {
            PoiCategory::Water => "1.2km",
            PoiCategory::Campsite => "24km",
            PoiCategory::Accommodation => "18km",
            PoiCategory::Resupply => "2.4km",
            PoiCategory::Pharmacy => "6.8km",
            PoiCategory::BikeShop => "12km",
            PoiCategory::Train => "5km",
        });
        return;
    }
    let sample: &str = match field {
        F::Speed => "23.4",
        F::AvgSpeed => "19.2",
        F::DistDone => "42.5",
        F::DistToGo => "12.3",
        F::Climbed => "810",
        F::ToClimb => "95",
        F::Grade => "4%",
        F::Elevation => "1240",
        F::RideTime => "2:14:30",
        // The samples agree: 1 h 05 left from the 14:32 clock sample gives 15:37.
        F::TimeToGo => "1:05",
        F::Eta => "15:37",
        F::TripToGo => "143",
        F::Clock => "14:32",
        F::NextWaypoint => {
            // The wide waypoint tile is a name caption + a right-aligned distance value.
            cell.caption.clear();
            let _ = cell.caption.push_str("Pass Summit");
            "8.7km"
        }
        // `waypoint_panel_ghost` draws the page-sized panel, not this caption and value pair.
        F::WaypointList => return,
        F::HeartRate => "152",
        F::Power => "210",
        F::Cadence => "88",
        // Handled above. They are spelled out to keep the match exhaustive.
        F::NextWater | F::NextCampsite | F::NextLodging | F::NextResupply | F::NextPharmacy | F::NextBikeShop => return,
    };
    cell.value.clear();
    let _ = cell.value.push_str(sample);
}

/// Draw the arrows that show a grabbed tile can be moved.
fn move_arrows(cv: &mut impl Surface, area: Rectangle) {
    use crate::screen::palette::ON_ACCENT;
    let x = area.top_left.x + area.size.width as i32 - 16;
    let midy = area.top_left.y + area.size.height as i32 / 2;
    cv.triangle(Point::new(x - 7, midy - 3), Point::new(x + 7, midy - 3), Point::new(x, midy - 12), ON_ACCENT);
    cv.triangle(Point::new(x - 7, midy + 3), Point::new(x + 7, midy + 3), Point::new(x, midy + 12), ON_ACCENT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::Activity;
    use crate::screen::test_ctx;
    use crate::{AppState, Mode, Settings};

    fn run(scr: &mut StatFieldsScreen, s: &mut Settings, g: Gesture) -> Transition {
        let mut st = AppState::new(0, 0, 1.0);
        let mut act = Activity::new(Mode::Idle);
        let mut cx = test_ctx(&mut st, &mut act, s);
        scr.handle(g, &mut cx)
    }

    #[test]
    fn grab_move_drop_reorders() {
        let mut s = Settings::default();
        let mut scr = StatFieldsScreen::new();
        run(&mut scr, &mut s, Gesture::Press);
        assert!(scr.grabbed);
        let first_before = s.stat_fields.as_slice()[0];
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, 1, "the cursor follows the grabbed field");
        assert_eq!(s.stat_fields.as_slice()[1], first_before, "the field moved down a slot");
        run(&mut scr, &mut s, Gesture::Press);
        assert!(!scr.grabbed);
    }

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

    #[test]
    fn deleting_the_last_field_lands_on_the_add_row() {
        let mut s = Settings::default();
        let mut scr = StatFieldsScreen::new();
        let len = s.stat_fields.len();
        for _ in 0..len - 1 {
            run(&mut scr, &mut s, Gesture::Step(1));
        }
        assert_eq!(scr.selected, len - 1);
        run(&mut scr, &mut s, Gesture::Hold);
        assert_eq!(scr.selected, s.stat_fields.len(), "cursor clamps to the Add row");
    }

    #[test]
    fn add_row_opens_picker_and_back_pops() {
        let mut s = Settings::default();
        let len = s.stat_fields.len();
        let mut scr = StatFieldsScreen::new();
        run(&mut scr, &mut s, Gesture::Step(len as i32));
        assert_eq!(scr.selected, len);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Press), Transition::Push(Screen::AddField(_))));
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop));
    }

    #[test]
    fn back_drops_a_grab_first() {
        let mut s = Settings::default();
        let mut scr = StatFieldsScreen::new();
        run(&mut scr, &mut s, Gesture::Press);
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::None));
        assert!(!scr.grabbed, "back dropped the grab, didn't pop");
        assert!(matches!(run(&mut scr, &mut s, Gesture::Back), Transition::Pop), "a second back pops");
    }

    #[test]
    fn grabbing_the_panel_moves_it_page_to_page() {
        use crate::stat_fields::StatField;
        let mut s = Settings::default();
        assert!(s.stat_fields.push(StatField::WaypointList));
        let panel_idx = s.stat_fields.len() - 1;
        let mut scr = StatFieldsScreen::new();
        for _ in 0..panel_idx {
            run(&mut scr, &mut s, Gesture::Step(1));
        }
        assert_eq!(scr.selected, panel_idx);
        assert_eq!(stat_fields::slot_of(&s.stat_fields, panel_idx).unwrap() / SLOTS_PER_PAGE, 1, "panel on page 1");
        run(&mut scr, &mut s, Gesture::Press);
        assert!(scr.grabbed);
        run(&mut scr, &mut s, Gesture::Step(-1));
        assert_eq!(scr.selected, 0, "the cursor follows the panel to page 0");
        assert_eq!(s.stat_fields.as_slice()[0], StatField::WaypointList);
        assert_eq!(stat_fields::slot_of(&s.stat_fields, 0).unwrap() / SLOTS_PER_PAGE, 0, "now on page 0");
        run(&mut scr, &mut s, Gesture::Step(1));
        assert_eq!(scr.selected, panel_idx, "and back down a page");
        assert_eq!(s.stat_fields.as_slice()[panel_idx], StatField::WaypointList);
    }

    #[test]
    fn category_tiles_ghost_a_localized_name_and_a_sample_distance() {
        use crate::next_ahead::NextAhead;
        use crate::settings::Language;
        use crate::stat_fields::{Readout, StatField};
        let mut navigation = crate::navigator::RouteState::new();
        navigation.active_route = Some(0);
        let cache = NextAhead::new();
        let wpts = obc_route::Waypoints::new();
        let recorder = crate::recorder::RecorderMachine::new();
        let cx = Readout {
            fix: None,
            navigation: &navigation,
            recorder: &recorder,
            units: crate::Units::Metric,
            route: None,
            profile: None,
            climb: None,
            waypoints: &wpts,
            next_waypoint: None,
            now: crate::settings::DateTime::default(),
            now_ms: 0,
            bike_type: crate::settings::BikeType::Road,
            language: Language::De,
            next_ahead: &cache,
            trip_later_m: None,
        };
        let mut seen: std::vec::Vec<(std::string::String, std::string::String)> = std::vec::Vec::new();
        for f in StatField::ALL.into_iter().filter(|f| f.category().is_some()) {
            let mut cell = f.cell(&cx);
            ghost_value(f, &mut cell, Language::De);
            assert_eq!(cell.caption.as_str(), f.name(Language::De), "the ghost caption is the localized category");
            assert!(cell.value.as_str().ends_with("km"), "and the sample is a plausible distance");
            seen.push((cell.caption.as_str().into(), cell.value.as_str().into()));
        }
        assert_eq!(seen.len(), 6);
        assert_eq!(seen[0].0, "Wasser", "German, not an English placeholder");
        let mut distances: std::vec::Vec<&str> = seen.iter().map(|(_, v)| v.as_str()).collect();
        distances.sort_unstable();
        distances.dedup();
        assert_eq!(distances.len(), 6, "each category gets its own sample, so a page of them isn't copy-paste");
    }

    #[test]
    fn cursor_page_follows_the_placement_walk() {
        let mut s = Settings::default();
        s.stat_fields.push(crate::stat_fields::StatField::Clock);
        let list = s.stat_fields;
        for i in 0..6 {
            assert_eq!(stat_fields::slot_of(&list, i).unwrap() / SLOTS_PER_PAGE, 0, "field {i} is on page 1");
        }
        assert_eq!(stat_fields::slot_of(&list, 6).unwrap() / SLOTS_PER_PAGE, 1, "the clock starts page 2");
        assert_eq!(stat_fields::next_free_slot(&list) / SLOTS_PER_PAGE, 1, "the Add ghost shares page 2");
        assert_eq!(stat_fields::slot_of(&list, 7), None, "past the selection there is no slot");
    }
}
