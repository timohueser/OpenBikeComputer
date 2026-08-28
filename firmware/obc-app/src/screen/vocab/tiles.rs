//! The shared **tile** vocabulary — the rounded stat panes of the riding grid and the Fields
//! editor, the `Next: <category>` variant, the waypoint list panel, and the caption fitter they
//! all truncate through.

use embedded_graphics::{prelude::Point, primitives::Rectangle};
use obc_render::{
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::screen::{palette, poi_menu};
use crate::{t, Msg};

/// Draw one stat tile — a rounded pane in `bg` with an olive caption over a big `value_color` Display
/// value (`INK` on the live riding grid; the olive `SUBTEXT` for the Fields editor's ghost sample
/// values, T8 item 4), optionally prefixed by an up-triangle for climb figures (the panel font has no
/// ↑ glyph). The
/// value sits at `value_align` (Left for the number-only fields; Right for the wide `NextWaypoint`
/// distance, so it hugs the far edge clear of the name caption). Shared by the riding Statistics
/// grid (tan panes) and the Fields editor (which draws the same tiles, amber under the cursor). The
/// caption+value block is vertically centred, so the taller editor tiles and the chart-squeezed
/// Statistics tiles both balance.
#[allow(clippy::too_many_arguments)] // a plain draw helper: surface + rect + caption/value + style
pub(crate) fn tile(
    cv: &mut impl Surface,
    area: Rectangle,
    label: &str,
    value: &str,
    arrow: bool,
    value_align: TextAlign,
    bg: u16,
    value_color: u16,
) {
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    cv.round(area, 5, bg);
    // Content block: Label caption (cap 18) + Display value (cap 26) with the same 18 px lead the
    // Statistics grid always had; centre it in whatever height the pane has.
    let cy = y + ((area.size.height as i32 - 48) / 2).max(4);
    // A caption wider than the tile (a long waypoint name) is truncated with an ASCII ellipsis; the
    // short unit captions of every built-in field pass through untouched. Caption inset less than
    // the value so those unit captions sit nearer the tile centre.
    let mut label_buf: heapless::String<24> = heapless::String::new();
    let label = fit_caption(label, area.size.width as i32 - 5, &mut label_buf, Font::Label);
    cv.text(label, Point::new(x + 5, cy), Font::Label, TextAlign::Left, SUBTEXT);
    let vy = cy + 18;
    match value_align {
        // Right-aligned (the wide waypoint distance): anchor at the tile's far edge, so it can never
        // collide with the caption on the line above.
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
                // Up-triangle sized to sit alongside the Display digits (dimmed with the value in the
                // Fields editor's ghost tiles).
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

/// Left inset of the category icon's centre inside a `Next: <category>` tile — half the ~22 px icon
/// box plus the tile's own 5 px caption inset, so the glyph sits on the same left margin the plain
/// tiles' captions do.
const CATEGORY_TILE_ICON_CX: i32 = 16;
/// Where a `Next: <category>` tile's caption starts: clear of the icon box, with a hair of air.
const CATEGORY_TILE_NAME_X: i32 = 31;

/// Draw a **`Next: <category>` tile** (epic #946, U5) — [`tile`]'s wide anatomy with the category's
/// row icon in front of the caption: `[icon] name` over a right-aligned Display distance. The name
/// is the nearest entry of that category ahead (a map POI or the rider's own categorized waypoint —
/// the tile can't tell, and deliberately doesn't say: on a stat page the answer is *how far*, and
/// provenance is the Up-ahead list's job); `--` when nothing of the kind is ahead, with the caption
/// falling back to the category's own name so the tile still reads as an answer rather than a blank.
///
/// Split out from [`tile`] rather than folded into it as a ninth argument: the icon changes the
/// caption's *geometry* (its inset and therefore its ellipsis budget), which every other tile would
/// have to opt out of. Same rounded pane, same caption/value fonts, same vertical centring, so the
/// two read as one system on the grid.
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
    // The caption/value block, centred in the pane exactly as `tile` centres its own.
    let cy = y + ((area.size.height as i32 - 48) / 2).max(4);
    poi_menu::draw_category_icon(cv, cat, Point::new(x + CATEGORY_TILE_ICON_CX, cy + 9), SUBTEXT, bg);
    let mut buf: heapless::String<24> = heapless::String::new();
    let name = fit_caption(name, w - CATEGORY_TILE_NAME_X - 5, &mut buf, Font::Label);
    cv.text(name, Point::new(x + CATEGORY_TILE_NAME_X, cy), Font::Label, TextAlign::Left, SUBTEXT);
    cv.text(value, Point::new(x + w - 8, cy + 18), Font::Display, TextAlign::Right, value_color);
}

/// Number of waypoint rows the 2×3 panel lists — the next this-many ahead of the rider.
pub(crate) const WAYPOINT_PANEL_ROWS: usize = 4;

/// Draw the **waypoint list panel** — the page-sized (2-col × 3-row) multi-row stat field
/// ([`WaypointList`](crate::stat_fields::StatField::WaypointList)). Its 2×3 list doesn't fit the
/// caption+value shape [`tile`] draws, so the Statistics grid and the Fields editor special-case
/// `rows() > 1` and call this instead (WYSIWYG: the editor draws the real panel, live). Chrome
/// matches [`tile`] — a rounded pane in `bg` with the olive `WAYPOINTS` caption — so it reads as one
/// system with the tan tiles around it.
///
/// Content is the next [`WAYPOINT_PANEL_ROWS`] waypoints ahead (rows `k..k+4` from
/// [`next_waypoint`](crate::stat_fields::Readout), the App-resolved first-ahead index): each row is
/// the name on the left and the along-route distance-to-go (`dist_along_m − progress`, clamped
/// through the pass-linger by `saturating_sub`) on the right, the **first row emphasized**
/// ([`Font::Body`]; the rest [`Font::Label`]). A name that would reach the distance column is
/// ellipsis-truncated. Fewer than four remaining leaves the tail rows blank; no route / nothing ahead
/// draws the frame + caption with a centred `--` (the route-relative fallback, like the 2×1 tile).
pub(crate) fn waypoint_panel(cv: &mut impl Surface, area: Rectangle, cx: &crate::stat_fields::Readout, bg: u16) {
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    let (w, hgt) = (area.size.width as i32, area.size.height as i32);
    cv.round(area, 5, bg);
    cv.text(t(Msg::TileWaypoints, cx.language), Point::new(x + 8, y + 8), Font::Label, TextAlign::Left, SUBTEXT);

    // The first waypoint ahead, guarded against a stale/out-of-range resolver index and the empty
    // table (no route loaded) — either way the panel falls back to a centred `--`.
    let ahead = cx.next_waypoint.filter(|&k| k < cx.waypoints.as_slice().len());
    let Some(k) = ahead else {
        cv.text("--", Point::new(x + w / 2, y + hgt / 2 - 11), Font::Body, TextAlign::Center, INK);
        return;
    };

    // Rows below the caption band, split evenly; the first is emphasized (Body), the rest Label.
    const HEAD: i32 = 30;
    let stride = (hgt - HEAD - 6) / WAYPOINT_PANEL_ROWS as i32;
    let wps = cx.waypoints.as_slice();
    for i in 0..WAYPOINT_PANEL_ROWS {
        let Some(wp) = wps.get(k + i) else { break }; // fewer than four remaining → blank tail rows
        let font = if i == 0 { Font::Body } else { Font::Label };
        let ry = y + HEAD + i as i32 * stride;
        // Distance-to-go, right-aligned at the far edge; the name is truncated clear of it.
        let dist = super::fmt::distance_short(wp.dist_along_m.saturating_sub(cx.activity.progress_m), cx.units);
        cv.text(&dist, Point::new(x + w - 10, ry), font, TextAlign::Right, INK);
        let budget = w - 20 - text_width(&dist, font) as i32 - 8;
        let mut buf: heapless::String<24> = heapless::String::new();
        let name = fit_caption(wp.name.as_str(), budget, &mut buf, font);
        cv.text(name, Point::new(x + 10, ry), font, TextAlign::Left, INK);
    }
}

/// The **Fields-editor ghost** of [`waypoint_panel`] (T8 item 4). In the editor there's no route
/// loaded, so the real panel would read a lone `--`; like the ghost sample values the tiles show, it
/// draws two fixed sample rows (`Brunnen  1.2km` emphasized [`Font::Body`], `Pass Summit  8.7km`
/// [`Font::Label`]) in the olive `SUBTEXT` — so the placed panel is judged against realistic content,
/// not a dash. Editor-only: the live Statistics grid always calls [`waypoint_panel`]. Chrome (the
/// rounded pane + olive `WAYPOINTS` caption) matches it so the two read as one system.
pub(crate) fn waypoint_panel_ghost(cv: &mut impl Surface, area: Rectangle, lang: crate::settings::Language, bg: u16) {
    use palette::*;
    let (x, y) = (area.top_left.x, area.top_left.y);
    let (w, hgt) = (area.size.width as i32, area.size.height as i32);
    cv.round(area, 5, bg);
    cv.text(t(Msg::TileWaypoints, lang), Point::new(x + 8, y + 8), Font::Label, TextAlign::Left, SUBTEXT);
    const HEAD: i32 = 30;
    let stride = (hgt - HEAD - 6) / WAYPOINT_PANEL_ROWS as i32;
    // Two sample waypoints ahead — name left, along-route distance-to-go right, the first emphasized;
    // all in olive so the block reads as a placeholder preview, not live content.
    let samples: [(&str, &str); 2] = [("Brunnen", "1.2km"), ("Pass Summit", "8.7km")];
    for (i, (name, dist)) in samples.iter().enumerate() {
        let font = if i == 0 { Font::Body } else { Font::Label };
        let ry = y + HEAD + i as i32 * stride;
        cv.text(dist, Point::new(x + w - 10, ry), font, TextAlign::Right, SUBTEXT);
        cv.text(name, Point::new(x + 10, ry), font, TextAlign::Left, SUBTEXT);
    }
}

/// Fit a caption into `budget_px` at `font`, dropping trailing chars and appending an ASCII ellipsis
/// (`...` — the device font is printable-ASCII only, so `…` would render as tofu) when it overflows.
/// Every built-in field's unit caption fits whole; only a long waypoint name is ever truncated (the
/// wide tile's caption at [`Font::Label`], the panel's per-row names at their row font). Writes into
/// `buf` and returns it. Pure integer geometry over the monospace cell width, so the truncation is
/// deterministic. Mirrors the Map chip's `fit_name`.
pub(crate) fn fit_caption<'b>(label: &str, budget_px: i32, buf: &'b mut heapless::String<24>, font: Font) -> &'b str {
    buf.clear();
    let char_w = font.char_width() as i32;
    if label.chars().count() as i32 * char_w <= budget_px {
        let _ = buf.push_str(label); // fits whole (caption ≤ StatCell cap ≤ buf)
        return buf.as_str();
    }
    const ELL: &str = "...";
    let keep = ((budget_px - ELL.len() as i32 * char_w) / char_w).max(0) as usize;
    for ch in label.chars().take(keep) {
        if buf.push(ch).is_err() {
            break;
        }
    }
    // A cut that lands on a word gap would read as `Fontaine du ...` — the space between the last
    // word and the ellipsis makes the truncation look like a typo. Drop trailing blanks first (the
    // budget only ever shrinks, so this can't overflow).
    while buf.ends_with(' ') {
        buf.pop();
    }
    let _ = buf.push_str(ELL);
    buf.as_str()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::harness::support::wpts;
    use crate::settings::{DateTime, Units};
    use obc_render::rect;
    use obc_route::Waypoints;

    /// A draw target that records only its text draws — the panel-content tests observe which strings
    /// land, at what font + alignment, ignoring the chrome primitives (fills/rounds).
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

    /// A bare metric readout over `activity` + `waypoints`, resolving `next` as the first waypoint
    /// ahead — enough for the panel drawer (which reads only those three).
    /// An empty per-category cache (U5): the panel drawer never reads it, but `Readout` carries it.
    static EMPTY_CACHE: &crate::next_ahead::NextAhead = &crate::next_ahead::NextAhead::EMPTY;

    /// A ride that has recorded nothing. A `static`, like the empty caches beside it, so the
    /// borrowed `Readout` outlives the call without a leak per test.
    fn idle_recorder() -> &'static crate::recorder::RecorderMachine {
        static IDLE: std::sync::LazyLock<crate::recorder::RecorderMachine> =
            std::sync::LazyLock::new(crate::recorder::RecorderMachine::new);
        &IDLE
    }

    fn readout<'a>(
        activity: &'a Activity,
        recorder: &'a crate::recorder::RecorderMachine,
        waypoints: &'a Waypoints,
        next: Option<usize>,
    ) -> crate::stat_fields::Readout<'a> {
        crate::stat_fields::Readout {
            fix: None,
            activity,
            recorder,
            units: Units::Metric,
            route: None,
            profile: None,
            climb: None,
            waypoints,
            next_waypoint: next,
            now: DateTime::default(),
            now_ms: 0,
            bike_profile_idx: 0,
            language: crate::settings::Language::En,
            next_ahead: EMPTY_CACHE,
        }
    }

    /// A representative panel rect (the Statistics grid's full-page area on the 240×320 panel).
    fn panel_area() -> Rectangle {
        rect(12, 136, 216, 174)
    }

    /// The panel pins the next four waypoints ahead (rows `k..k+4`), the first emphasized (`Body`)
    /// and the rest `Label`, each row a right-aligned distance-to-go (`dist_along_m − progress`) and
    /// a left name; with only two remaining, the tail rows stay blank (nothing drawn).
    #[test]
    fn waypoint_panel_pins_the_next_four_and_blanks_the_tail() {
        let act = Activity::new(Mode::Riding); // progress 0
        let w = wpts(&[(1_000, "Brunnen"), (5_000, "Alp")]); // short names → verbatim, no truncation
        let cx = readout(&act, idle_recorder(), &w, Some(0));
        let mut rec = TextRec::default();
        waypoint_panel(&mut rec, panel_area(), &cx, palette::PARCHMENT_SHADE);

        // caption, then per row: distance (right) then name (left). Two waypoints → 1 + 2×2 = 5.
        assert_eq!(rec.calls.len(), 5, "caption + two rows; the two empty tail rows draw nothing");
        assert_eq!((rec.calls[0].0.as_str(), rec.calls[0].1), ("WAYPOINTS", Font::Label));
        // Row 0 — emphasized (Body), distance-to-go 1000 − 0 = 1.0 km, then the name.
        assert_eq!((rec.calls[1].0.as_str(), rec.calls[1].1, rec.calls[1].2), ("1.0km", Font::Body, TextAlign::Right));
        assert_eq!((rec.calls[2].0.as_str(), rec.calls[2].1, rec.calls[2].2), ("Brunnen", Font::Body, TextAlign::Left));
        // Row 1 — Label, 5000 − 0 = 5.0 km.
        assert_eq!((rec.calls[3].0.as_str(), rec.calls[3].1, rec.calls[3].2), ("5.0km", Font::Label, TextAlign::Right));
        assert_eq!((rec.calls[4].0.as_str(), rec.calls[4].1, rec.calls[4].2), ("Alp", Font::Label, TextAlign::Left));
    }

    /// A name too wide for the space left of its distance is ellipsis-truncated (ASCII `...`) so it
    /// can never run into the distance column — the panel row's version of the tile's `fit_caption`.
    #[test]
    fn waypoint_panel_truncates_a_long_name_before_the_distance() {
        let act = Activity::new(Mode::Riding);
        let w = wpts(&[(12_400, "Pass Summit Overlook")]); // 20 chars ≤ WAYPOINT_NAME_CAP, too wide for the row
        let cx = readout(&act, idle_recorder(), &w, Some(0));
        let mut rec = TextRec::default();
        waypoint_panel(&mut rec, panel_area(), &cx, palette::PARCHMENT_SHADE);
        // Row 0: distance then the truncated name.
        assert_eq!(rec.calls[1].0.as_str(), "12.4km", "the distance-to-go is intact");
        let name = rec.calls[2].0.as_str();
        assert!(name.ends_with("..."), "an over-long name is ellipsis-truncated, got {name:?}");
        assert!(name.starts_with("Pass"), "…keeping its leading characters, got {name:?}");
        // And the truncated name plus a gap stays clear of the distance's left edge.
        let name_px = text_width(name, Font::Body) as i32;
        let budget = panel_area().size.width as i32 - 20 - text_width("12.4km", Font::Body) as i32 - 8;
        assert!(name_px <= budget, "the truncated name fits its budget ({name_px} <= {budget})");
    }

    /// Inside the 100 m pass-linger (progress past the still-current first waypoint) the row-1
    /// distance clamps to `0m` via `saturating_sub` — the "you are here" readout the 2×1 tile shares.
    #[test]
    fn waypoint_panel_row_one_clamps_to_zero_in_the_linger() {
        let mut act = Activity::new(Mode::Riding);
        act.progress_m = 1_050; // 50 m past Brunnen, still its index (inside the linger)
        let w = wpts(&[(1_000, "Brunnen"), (5_000, "Pass Summit")]);
        let cx = readout(&act, idle_recorder(), &w, Some(0));
        let mut rec = TextRec::default();
        waypoint_panel(&mut rec, panel_area(), &cx, palette::PARCHMENT_SHADE);
        assert_eq!(rec.calls[1].0.as_str(), "0m", "the passed first waypoint clamps to 0m");
        assert_eq!(rec.calls[2].0.as_str(), "Brunnen");
    }

    /// Empty state — the frame + caption `WAYPOINTS` and a single centred `--` — for every way there's
    /// nothing ahead: no index resolved, a stale out-of-range index, and an empty table.
    #[test]
    fn waypoint_panel_empty_state_is_a_centred_dash() {
        let act = Activity::new(Mode::Riding);
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

    /// A draw target that records text **with its anchor** — the `Next: <category>` tile's whole
    /// point is *where* the two strings land (the caption clear of the icon, the value on the far
    /// edge), which the font/align-only recorder above can't see. Primitives are counted, since the
    /// category icon is drawn, not typed.
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

    /// The `Next: <category>` tile's anatomy (epic #946, U5): the category icon is drawn (not
    /// typed), the name sits clear of it in `Label`, and the distance hugs the far edge in the big
    /// `Display` face — the wide next-waypoint tile's shape, plus the glyph.
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

    /// A name too long for the tile is ellipsized against the **icon-narrowed** budget, and the cut
    /// never leaves a dangling space before the ellipsis.
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
        assert!(name.ends_with("..."), "an over-long name is cut with the house ellipsis, got {name:?}");
        assert!(!name.ends_with(" ..."), "…and never with a dangling space before it");
        let budget = 220 - CATEGORY_TILE_NAME_X - 5;
        assert!(text_width(name, Font::Label) as i32 <= budget, "the cut stays inside the icon-narrowed budget");
    }

    /// A stat tile's caption fits its pixel budget: a short built-in caption passes through verbatim,
    /// a long waypoint name is cut to leading chars + an ASCII ellipsis that stays within budget — so
    /// the wide `NextWaypoint` tile's name can never run into its right-aligned value.
    #[test]
    fn tile_caption_truncation_fits_the_budget() {
        let cw = Font::Label.char_width() as i32;
        let mut buf = heapless::String::<24>::new();
        assert_eq!(
            fit_caption("NEXT WPT", 100 * cw, &mut buf, Font::Label),
            "NEXT WPT",
            "a caption within budget is verbatim"
        );
        let mut buf = heapless::String::<24>::new();
        let fitted = fit_caption("Pass Summit Overlook", 10 * cw, &mut buf, Font::Label);
        assert_eq!(fitted, "Pass Su...", "7 leading chars + ellipsis fill the 10-cell budget");
        assert!(text_width(fitted, Font::Label) as i32 <= 10 * cw, "and it stays within budget");
    }
}
