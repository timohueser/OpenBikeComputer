//! The shared tile vocabulary: the rounded stat panes of the riding grid and the Fields editor,
//! the `Next: <category>` variant, and the waypoint list panel.

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use super::marquee::{fit, MarqueeFrame};
use crate::screen::route_overview::{climb_arrow, ARROW_W};
use crate::screen::{palette, poi_menu};
use crate::{t, Msg};

/// Draw one stat tile: a rounded pane in `bg` with a caption over a big `value_color` value. Set
/// `arrow` to prefix a climb figure with an up-triangle, because the panel font has no ↑ glyph.
/// `value_align` is Right for a wide value, so it hugs the far edge clear of the caption. A
/// `caption_climb` follows the caption behind a climb arrow. The caption and value block is
/// vertically centred, whatever height the pane has.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tile(
    cv: &mut impl Surface,
    area: Rectangle,
    marquee: &MarqueeFrame,
    label: &str,
    caption_climb: Option<&str>,
    value: &str,
    arrow: bool,
    value_align: TextAlign,
    bg: u16,
    value_color: u16,
) {
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    cv.round(area, 5, bg);
    let cy = y + ((area.size.height as i32 - 48) / 2).max(4);
    // A caption wider than the tile scrolls once when it changes and then rests on its head. It
    // must never move perpetually while riding.
    let caption_budget = area.size.width as i32 - 5;
    let caption_row = rect(x + 5, cy, caption_budget, Font::Label.line_height() as i32);
    let label = marquee.fit_once(label, caption_budget, Font::Label, caption_row);
    cv.text(&label, Point::new(x + 5, cy), Font::Label, TextAlign::Left, SUBTEXT);
    if let Some(climb) = caption_climb {
        let ax = x + 5 + text_width(&label, Font::Label) as i32 + 6;
        climb_arrow(cv, ax, cy, true, SUBTEXT);
        cv.text(climb, Point::new(ax + ARROW_W + 3, cy), Font::Label, TextAlign::Left, SUBTEXT);
    }
    let vy = cy + 18;
    match value_align {
        TextAlign::Right => {
            cv.text(
                value,
                Point::new(x + area.size.width as i32 - 8, vy),
                Font::Display,
                TextAlign::Right,
                value_color,
            );
        }
        _ => {
            let vx = if arrow {
                let ax = x + 8;
                cv.triangle(
                    Point::new(ax, vy + 26),
                    Point::new(ax + 13, vy + 26),
                    Point::new(ax + 6, vy + 6),
                    value_color,
                );
                x + 26
            } else {
                x + 8
            };
            cv.text(value, Point::new(vx, vy), Font::Display, TextAlign::Left, value_color);
        }
    }
}

/// Left inset of the category icon's centre inside a `Next: <category>` tile, so the glyph sits on
/// the same left margin as a plain tile's caption.
const CATEGORY_TILE_ICON_CX: i32 = 16;
/// Where a `Next: <category>` tile's caption starts, clear of the icon box.
const CATEGORY_TILE_NAME_X: i32 = 31;

/// Draw a `Next: <category>` tile: [`tile`]'s wide anatomy with the category's row icon in front of
/// the caption, as `[icon] name` over a right-aligned distance. It is separate from [`tile`],
/// because the icon changes the caption's inset and therefore its ellipsis budget.
pub(crate) fn category_tile(
    cv: &mut impl Surface,
    area: Rectangle,
    cat: obc_reader::PoiCategory,
    name: &str,
    value: &str,
    bg: u16,
    value_color: u16,
) {
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    let w = area.size.width as i32;
    cv.round(area, 5, bg);
    let cy = y + ((area.size.height as i32 - 48) / 2).max(4);
    poi_menu::draw_category_icon(cv, cat, Point::new(x + CATEGORY_TILE_ICON_CX, cy + 9), SUBTEXT, bg);
    let name = fit(name, w - CATEGORY_TILE_NAME_X - 5, Font::Label);
    cv.text(&name, Point::new(x + CATEGORY_TILE_NAME_X, cy), Font::Label, TextAlign::Left, SUBTEXT);
    cv.text(value, Point::new(x + w - 8, cy + 18), Font::Display, TextAlign::Right, value_color);
}

/// Number of waypoint rows the panel lists, counted ahead of the rider.
pub(crate) const WAYPOINT_PANEL_ROWS: usize = 4;

/// Draw the waypoint list panel: the page-sized multi-row stat field
/// ([`WaypointList`](crate::stat_fields::StatField::WaypointList)). Its list does not fit the
/// caption and value shape [`tile`] draws, so the callers special-case `rows() > 1`.
///
/// The content is the next [`WAYPOINT_PANEL_ROWS`] waypoints ahead: a name on the left and the
/// along-route distance to go on the right, the first row emphasized. A name that would reach the
/// distance column is cut. Nothing ahead draws the frame, the caption and a centred `--`.
pub(crate) fn waypoint_panel(cv: &mut impl Surface, area: Rectangle, cx: &crate::stat_fields::Readout, bg: u16) {
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    let (w, hgt) = (area.size.width as i32, area.size.height as i32);
    cv.round(area, 5, bg);
    cv.text(t(Msg::TileWaypoints, cx.language), Point::new(x + 8, y + 8), Font::Label, TextAlign::Left, SUBTEXT);

    // Guarded against a stale resolver index and an empty table.
    let ahead = cx.next_waypoint.filter(|&k| k < cx.waypoints.as_slice().len());
    let Some(k) = ahead else {
        cv.text("--", Point::new(x + w / 2, y + hgt / 2 - 11), Font::Body, TextAlign::Center, INK);
        return;
    };

    const HEAD: i32 = 30;
    let stride = (hgt - HEAD - 6) / WAYPOINT_PANEL_ROWS as i32;
    let wps = cx.waypoints.as_slice();
    for i in 0..WAYPOINT_PANEL_ROWS {
        let Some(wp) = wps.get(k + i) else { break }; // fewer remaining: the tail rows stay blank
        let font = if i == 0 { Font::Body } else { Font::Label };
        let ry = y + HEAD + i as i32 * stride;
        let dist = super::fmt::distance_short(wp.dist_along_m.saturating_sub(cx.navigation.progress_m), cx.units);
        cv.text(&dist, Point::new(x + w - 10, ry), font, TextAlign::Right, INK);
        let budget = w - 20 - text_width(&dist, font) as i32 - 8;
        let name = fit(wp.name.as_str(), budget, font);
        cv.text(&name, Point::new(x + 10, ry), font, TextAlign::Left, INK);
    }
}

/// The Fields-editor ghost of [`waypoint_panel`]. The editor has no route loaded, so the real panel
/// would read a lone `--`. This draws two fixed sample rows instead, so the placed panel is judged
/// against realistic content.
pub(crate) fn waypoint_panel_ghost(cv: &mut impl Surface, area: Rectangle, lang: crate::settings::Language, bg: u16) {
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    let (w, hgt) = (area.size.width as i32, area.size.height as i32);
    cv.round(area, 5, bg);
    cv.text(t(Msg::TileWaypoints, lang), Point::new(x + 8, y + 8), Font::Label, TextAlign::Left, SUBTEXT);
    const HEAD: i32 = 30;
    let stride = (hgt - HEAD - 6) / WAYPOINT_PANEL_ROWS as i32;
    // Olive, so the block reads as a placeholder preview and not as live content.
    let samples: [(&str, &str); 2] = [("Brunnen", "1.2km"), ("Pass Summit", "8.7km")];
    for (i, (name, dist)) in samples.iter().enumerate() {
        let font = if i == 0 { Font::Body } else { Font::Label };
        let ry = y + HEAD + i as i32 * stride;
        cv.text(dist, Point::new(x + w - 10, ry), font, TextAlign::Right, SUBTEXT);
        cv.text(name, Point::new(x + 10, ry), font, TextAlign::Left, SUBTEXT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::support::wpts;
    use crate::navigator::RouteState;
    use crate::settings::{DateTime, Units};
    use obc_render::rect;
    use obc_route::Waypoints;

    /// A draw target that records only its text draws, with the font and the alignment.
    #[derive(Default)]
    struct TextRec {
        calls: heapless::Vec<(heapless::String<24>, Font, TextAlign), 16>,
    }
    impl Surface for TextRec {
        fn clear(&mut self, _: u16) {}
        fn fill(&mut self, _: Rectangle, _: u16) {}
        fn round(&mut self, _: Rectangle, _: u32, _: u16) {}
        fn round_outline(&mut self, _: Rectangle, _: u32, _: u16) {}
        fn line(&mut self, _: Point, _: Point, _: u16) {}
        fn triangle(&mut self, _: Point, _: Point, _: Point, _: u16) {}
        fn disc(&mut self, _: Point, _: u32, _: u16) {}
        fn text(&mut self, s: &str, at: Point, font: Font, align: TextAlign, _: u16) -> Point {
            let mut buf = heapless::String::new();
            let _ = buf.push_str(s);
            let _ = self.calls.push((buf, font, align));
            at
        }
    }

    static EMPTY_CACHE: &crate::next_ahead::NextAhead = &crate::next_ahead::NextAhead::EMPTY;

    /// A ride that has recorded nothing. It is a `static`, so the borrowed `Readout` outlives the
    /// call.
    fn idle_recorder() -> &'static crate::recorder::RecorderMachine {
        static IDLE: std::sync::LazyLock<crate::recorder::RecorderMachine> =
            std::sync::LazyLock::new(crate::recorder::RecorderMachine::new);
        &IDLE
    }

    fn readout<'a>(
        navigation: &'a RouteState,
        recorder: &'a crate::recorder::RecorderMachine,
        waypoints: &'a Waypoints,
        next: Option<usize>,
    ) -> crate::stat_fields::Readout<'a> {
        crate::stat_fields::Readout {
            fix: None,
            navigation,
            recorder,
            units: Units::Metric,
            route: None,
            profile: None,
            climb: None,
            waypoints,
            next_waypoint: next,
            now: DateTime::default(),
            now_ms: 0,
            bike_type: crate::settings::BikeType::Road,
            language: crate::settings::Language::En,
            next_ahead: EMPTY_CACHE,
            trip: None,
        }
    }

    /// The Statistics grid's full-page area on the 240×320 panel.
    fn panel_area() -> Rectangle {
        rect(12, 136, 216, 174)
    }

    #[test]
    fn waypoint_panel_pins_the_next_four_and_blanks_the_tail() {
        let act = RouteState::new(); // progress 0
        let w = wpts(&[(1_000, "Brunnen"), (5_000, "Alp")]);
        let cx = readout(&act, idle_recorder(), &w, Some(0));
        let mut rec = TextRec::default();
        waypoint_panel(&mut rec, panel_area(), &cx, palette::PARCHMENT_SHADE);

        // The caption, then per row the distance and the name.
        assert_eq!(rec.calls.len(), 5, "caption + two rows; the two empty tail rows draw nothing");
        assert_eq!((rec.calls[0].0.as_str(), rec.calls[0].1), ("WAYPOINTS", Font::Label));
        assert_eq!((rec.calls[1].0.as_str(), rec.calls[1].1, rec.calls[1].2), ("1.0km", Font::Body, TextAlign::Right));
        assert_eq!((rec.calls[2].0.as_str(), rec.calls[2].1, rec.calls[2].2), ("Brunnen", Font::Body, TextAlign::Left));
        assert_eq!((rec.calls[3].0.as_str(), rec.calls[3].1, rec.calls[3].2), ("5.0km", Font::Label, TextAlign::Right));
        assert_eq!((rec.calls[4].0.as_str(), rec.calls[4].1, rec.calls[4].2), ("Alp", Font::Label, TextAlign::Left));
    }

    #[test]
    fn waypoint_panel_truncates_a_long_name_before_the_distance() {
        let act = RouteState::new();
        let w = wpts(&[(12_400, "Pass Summit Overlook")]); // too wide for the row
        let cx = readout(&act, idle_recorder(), &w, Some(0));
        let mut rec = TextRec::default();
        waypoint_panel(&mut rec, panel_area(), &cx, palette::PARCHMENT_SHADE);
        assert_eq!(rec.calls[1].0.as_str(), "12.4km", "the distance-to-go is intact");
        let name = rec.calls[2].0.as_str();
        assert!(name.ends_with(".."), "an over-long name is cut with the dots, got {name:?}");
        assert!(name.starts_with("Pass"), "…keeping its leading characters, got {name:?}");
        let name_px = text_width(name, Font::Body) as i32;
        let budget = panel_area().size.width as i32 - 20 - text_width("12.4km", Font::Body) as i32 - 8;
        assert!(name_px <= budget, "the truncated name fits its budget ({name_px} <= {budget})");
    }

    #[test]
    fn waypoint_panel_row_one_clamps_to_zero_in_the_linger() {
        let mut act = RouteState::new();
        act.progress_m = 1_050; // 50 m past Brunnen, still its index
        let w = wpts(&[(1_000, "Brunnen"), (5_000, "Pass Summit")]);
        let cx = readout(&act, idle_recorder(), &w, Some(0));
        let mut rec = TextRec::default();
        waypoint_panel(&mut rec, panel_area(), &cx, palette::PARCHMENT_SHADE);
        assert_eq!(rec.calls[1].0.as_str(), "0m", "the passed first waypoint clamps to 0m");
        assert_eq!(rec.calls[2].0.as_str(), "Brunnen");
    }

    #[test]
    fn waypoint_panel_empty_state_is_a_centred_dash() {
        let act = RouteState::new();
        let w = wpts(&[(1_000, "Brunnen")]);
        let empty = Waypoints::new();
        for cx in [
            readout(&act, idle_recorder(), &empty, None),    // no route / nothing ahead
            readout(&act, idle_recorder(), &w, Some(9)),     // a stale index past the table's end
            readout(&act, idle_recorder(), &empty, Some(0)), // an index against an empty table
        ] {
            let mut rec = TextRec::default();
            waypoint_panel(&mut rec, panel_area(), &cx, palette::PARCHMENT_SHADE);
            assert_eq!(rec.calls.len(), 2, "just the caption and the fallback dash — no rows");
            assert_eq!(rec.calls[0].0.as_str(), "WAYPOINTS");
            assert_eq!((rec.calls[1].0.as_str(), rec.calls[1].2), ("--", TextAlign::Center), "a centred fallback dash");
        }
    }

    /// A draw target that records text with its anchor, and counts the primitives the icon draws.
    #[derive(Default)]
    struct PosRec {
        calls: heapless::Vec<(heapless::String<24>, Point, Font, TextAlign), 8>,
        primitives: usize,
    }
    impl Surface for PosRec {
        fn clear(&mut self, _: u16) {}
        fn fill(&mut self, _: Rectangle, _: u16) {
            self.primitives += 1;
        }
        fn round(&mut self, _: Rectangle, _: u32, _: u16) {}
        fn round_outline(&mut self, _: Rectangle, _: u32, _: u16) {}
        fn line(&mut self, _: Point, _: Point, _: u16) {
            self.primitives += 1;
        }
        fn triangle(&mut self, _: Point, _: Point, _: Point, _: u16) {
            self.primitives += 1;
        }
        fn disc(&mut self, _: Point, _: u32, _: u16) {
            self.primitives += 1;
        }
        fn text(&mut self, s: &str, at: Point, font: Font, align: TextAlign, _: u16) -> Point {
            let mut buf = heapless::String::new();
            let _ = buf.push_str(s);
            let _ = self.calls.push((buf, at, font, align));
            at
        }
    }

    #[test]
    fn category_tile_draws_icon_name_and_a_right_aligned_distance() {
        let area = rect(10, 40, 220, 60);
        let mut cv = PosRec::default();
        category_tile(
            &mut cv,
            area,
            obc_reader::PoiCategory::Water,
            "Fontaine",
            "2.4km",
            palette::PARCHMENT_SHADE,
            palette::INK,
        );
        assert!(cv.primitives > 0, "the category glyph draws as primitives, not a font char");
        let (name, name_at, name_font, _) = &cv.calls[0];
        assert_eq!(name.as_str(), "Fontaine");
        assert_eq!(*name_font, Font::Label, "the name is a caption, like every other tile's");
        assert!(name_at.x >= area.top_left.x + CATEGORY_TILE_NAME_X, "…and starts clear of the icon box");
        let (value, value_at, value_font, value_align) = &cv.calls[1];
        assert_eq!(value.as_str(), "2.4km");
        assert_eq!(*value_font, Font::Display, "the distance is the glanceable number");
        assert_eq!(*value_align, TextAlign::Right);
        assert_eq!(value_at.x, area.top_left.x + area.size.width as i32 - 8, "anchored on the tile's far edge");
        assert!(value_at.y > name_at.y, "and below the name, never beside it");
    }

    #[test]
    fn category_tile_ellipsizes_against_the_icon_narrowed_budget() {
        let mut cv = PosRec::default();
        category_tile(
            &mut cv,
            rect(10, 40, 220, 60),
            obc_reader::PoiCategory::Resupply,
            "Boulangerie du Port Hercule",
            "1.6km",
            palette::PARCHMENT_SHADE,
            palette::INK,
        );
        let name = cv.calls[0].0.as_str();
        assert!(name.ends_with(".."), "an over-long name is cut with the house dots, got {name:?}");
        assert!(!name.ends_with(" .."), "…and never with a dangling space before them");
        let budget = 220 - CATEGORY_TILE_NAME_X - 5;
        assert!(text_width(name, Font::Label) as i32 <= budget, "the cut stays inside the icon-narrowed budget");
    }
}
