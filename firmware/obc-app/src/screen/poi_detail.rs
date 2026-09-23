//! Place detail with a cached schedule and live, trusted local-time status.
//! Prepare resolves the schedule into the App-owned scratch. Activation checks the current
//! source validity again before route planning.

use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_formats::obcm::poi_label_of;
use obc_reader::{hours::OpeningStatus, Interval, Poi, PoiCategory, WeeklySchedule};
use obc_render::{
    rect,
    text::{text_width, Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::poi_display::{draw_side_arrow, ARROW_W};
use super::poi_list::draw_bearing_arrow;
use super::vocab::chrome::{title_frame, LIST_TOP};
use super::vocab::fmt::write_distance_split;
use super::vocab::rows::{ledger_row, ledger_value_left};
use super::{palette, Ctx, Render, Transition};

/// The selected place; its schedule lives in App scratch to keep the screen union small.
#[derive(Debug)]
pub struct PoiDetailScreen {
    poi: Poi,
    /// The signed lateral offset from the route line (m; positive is right of the direction of
    /// travel), when the detail was opened from the [Up-ahead timeline](super::WhatsNextScreen).
    /// `None` from the nearby-POI browser, which has no route.
    off_route_m: Option<i16>,
    /// The first schedule read has completed, including missing data or an error.
    schedule_ready: bool,
    landmark_category: u8,
    pub(crate) visit_error: Option<crate::navigator::VisitUnavailable>,
}

impl PoiDetailScreen {
    /// Open the detail for `poi`. The schedule resolves on the first [`prepare`](Self::prepare)
    /// pass that has a `Reader`.
    pub fn new(poi: Poi) -> Self {
        PoiDetailScreen { poi, off_route_m: None, schedule_ready: false, visit_error: None, landmark_category: 0 }
    }

    pub(crate) fn is_landmark(&self) -> bool {
        self.landmark_category != 0
    }

    pub(crate) fn landmark(mut self, category: u8) -> Self {
        self.landmark_category = category;
        self
    }

    /// Carry the POI's signed lateral offset from the route (m) onto the detail.
    pub(crate) fn off_route(mut self, offset_m: i32) -> Self {
        self.off_route_m = Some(offset_m.clamp(i16::MIN as i32, i16::MAX as i32) as i16);
        self
    }

    /// Whether the schedule cache still needs a `Reader`. It drives
    /// [`base_needs_reader`](crate::App::base_needs_reader), so the host keeps the reader built
    /// until the hours read lands in `prepare`.
    pub(crate) fn hours_pending(&self, scratch: &super::PoiScratch) -> bool {
        self.visit_error != Some(crate::navigator::VisitUnavailable::SourceChanged)
            && (!self.schedule_ready || scratch.detail_source != self.poi.metadata.source.0)
    }

    pub(crate) fn invalidate_source(&mut self) {
        self.schedule_ready = true;
        self.visit_error = Some(crate::navigator::VisitUnavailable::SourceChanged);
    }

    pub(crate) fn poi(&self) -> &Poi {
        &self.poi
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        // App captures the exact source and live origin for Visit before screen dispatch.
        match g {
            Gesture::Back => Transition::Pop,
            _ => Transition::None,
        }
    }

    /// Resolve the POI's schedule into the App scratch, on the first prepare pass that has a
    /// `Reader`. This is the one place the side-effectful hours read runs; [`draw`](Self::draw)
    /// then only reads the cache.
    pub(crate) fn prepare(&mut self, px: &mut super::Prepare) {
        if !self.hours_pending(px.poi_scratch) {
            return;
        }
        let Some(reader) = px.reader else {
            return; // no map this frame — retry next prepare
        };
        let schedule = reader.try_poi_hours(self.poi.hours_ref);
        px.poi_scratch.detail_source = self.poi.metadata.source.0;
        px.poi_scratch.detail_valid = schedule.is_ok();
        px.poi_scratch.detail_schedule = schedule.ok().flatten();
        self.schedule_ready = true;
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;

        let (w, h) = (rx.w, rx.h);
        let category = obc_formats::obcm::poi_category_of(self.poi.subtype);
        let title = if self.landmark_category > 0 {
            Msg::AssistantLandmark
        } else {
            category.map_or(Msg::PoiDetailTitle, super::poi_menu::category_msg)
        };
        title_frame(cv, w, h, rx.t(title), "");

        // The subtype label is the subtitle, and the whole name line when the POI is unnamed.
        let label = if self.landmark_category > 0 {
            rx.t(super::landmarks::kind(self.landmark_category))
        } else {
            poi_label_of(self.poi.subtype).unwrap_or("POI")
        };
        let named = !self.poi.name.is_empty();
        let name = if named { self.poi.name.as_str() } else { label };

        // The category icon at the left inset, the name beside it. The name is never ellipsized: it
        // wraps to a second line, which runs under the icon's column.
        let x = 16;
        let name_top = LIST_TOP + 4;
        let mut name_x = x;
        if let Some(cat) = category {
            let icon_c = Point::new(x + 11, name_top + Font::Body.cap_mid() as i32);
            super::poi_menu::draw_category_icon(cv, cat, icon_c, INK, PARCHMENT);
            name_x = x + 22 + 8;
        }
        let name_bot = draw_wrapped(cv, name, name_x, name_top, w - name_x - 16, INK);

        let schedule = rx.poi_scratch.detail_schedule.filter(|s| {
            self.visit_error != Some(crate::navigator::VisitUnavailable::SourceChanged)
                && rx.poi_scratch.detail_source == self.poi.metadata.source.0
                && s.flags() == 0
        });
        let (heading, intervals) = hours_view(schedule.as_ref(), rx.place_local);
        // The OPEN / CLOSED badge, only when the POI has a schedule. The pill is sized from the
        // measured text, so accents in a translated label stay inside it.
        let badge = schedule.filter(|s| s.status(rx.place_local) != OpeningStatus::Unknown).map(|sched| {
            let open = sched.status(rx.place_local) == OpeningStatus::Open;
            let (text, bg) = if open { (rx.t(Msg::PoiDetailOpen), ON) } else { (rx.t(Msg::PoiDetailClosed), WARNING) };
            let ink = obc_render::text::text_ink_bounds(text, Font::Body).unwrap_or(0..0);
            let badge_h = ink.end - ink.start + 2 * BADGE_PAD_Y;
            (text, bg, ink, badge_h)
        });

        // A page that would reach the footer bar drops the subtitle, because the title already names
        // the category, and draws its hours in the compact rows.
        let subtitle_h = 6 + Font::Label.cap_bottom() as i32;
        let rows_h = ROW_PITCH * (1 + i32::from(self.off_route_m.is_some()));
        let full_top = if named { name_bot + subtitle_h } else { name_bot };
        let full_bottom =
            hours_bottom(full_top + 8 + rows_h, intervals.len(), intervals.len() > 2, badge.as_ref().map(|b| b.3));
        let tight = full_bottom + 6 > super::route_overview::start_button_top(h);

        // The subtitle is skipped when the name line is already the label, so it never repeats.
        let mut sub_bot = name_bot;
        if named && !tight {
            let sub_y = name_bot + 6;
            cv.text(label, Point::new(x, sub_y), Font::Label, TextAlign::Left, SUBTEXT);
            sub_bot = name_bot + subtitle_h;
        }

        // From the Up-ahead timeline the distance runs along the route and the offset has its own
        // row. Elsewhere it is the straight-line distance, with the 8-way arrow of the list rows
        // left of the value when there is a heading reference.
        let units = rx.settings.units;
        let mut y = sub_bot + 8;
        let caption = rx.t(if self.off_route_m.is_some() { Msg::PoiDetailAhead } else { Msg::PoiDetailAway });
        let value_left = distance_row(cv, w, y, caption, self.poi.distance_m, units);
        let bearing = (self.off_route_m, rx.state.user_fix, rx.state.effective_heading_deg());
        if let (None, Some(fix), Some(heading)) = bearing {
            draw_bearing_arrow(
                cv,
                Point::new(value_left - MARK_GAP - BEARING_R, y + VALUE_MID),
                BEARING_R,
                (fix.lon, fix.lat),
                (self.poi.lon, self.poi.lat),
                heading,
            );
        }
        y += ROW_PITCH;
        if let Some(off) = self.off_route_m {
            let caption = rx.t(Msg::PoiDetailOffRoute);
            let value_left = distance_row(cv, w, y, caption, off.unsigned_abs() as u32, units);
            draw_side_arrow(cv, Point::new(value_left - MARK_GAP - ARROW_W, y + VALUE_MID), off > 0, INK);
            y += ROW_PITCH;
        }

        // Today's hours: a heading row, then each open interval on its own row. An overnight
        // spillover can add a third range, which the compact rows still fit in the same area.
        let (head_y, mut row_y) = hours_top(y);
        // Drinking water has no opening hours, so "not listed" there would read as missing data.
        if !(matches!(heading, Msg::PoiDetailHoursNotListed) && category == Some(PoiCategory::Water)) {
            cv.text(rx.t(heading), Point::new(x, head_y), Font::Label, TextAlign::Left, SUBTEXT);
        }

        let (range_font, range_step) = range_face(intervals.len() > 2 || tight);
        for iv in &intervals {
            let mut range: heapless::String<16> = heapless::String::new();
            write_interval(&mut range, iv);
            cv.text(&range, Point::new(x, row_y), range_font, TextAlign::Left, INK);
            row_y += range_step;
        }

        // With interval rows on the page the badge rides the "Today" caption line, right-aligned,
        // so even a two-line name with two intervals clears the footer bar. The pill is taller than
        // the caption, so [`BADGE_RAISE`] lifts it clear of the first hours row. With no interval
        // rows the badge keeps its own spot under the caption, where a longer closed-today caption
        // would collide with a right-aligned pill.
        if let Some((text, bg, ink, badge_h)) = badge {
            let font = Font::Body;
            let badge_w = text_width(text, font) as i32 + 2 * BADGE_PAD_X;
            let (bx, badge_y) = if intervals.is_empty() {
                (x, row_y + BADGE_GAP)
            } else {
                (w - x - badge_w, head_y + Font::Label.cap_mid() as i32 - badge_h / 2 - BADGE_RAISE)
            };
            cv.round(rect(bx, badge_y, badge_w, badge_h), 6, bg);
            let ty = badge_y + BADGE_PAD_Y - ink.start;
            cv.text(text, Point::new(bx + badge_w / 2, ty), font, TextAlign::Center, PARCHMENT);
        }

        // The footer action row uses the same drawer as the Route overview's START RIDE bar, so the
        // two cannot drift. A press anywhere already opens the create-route confirm.
        let label = if self.visit_error == Some(crate::navigator::VisitUnavailable::SourceChanged) {
            rx.t(Msg::AssistantMapChanged)
        } else if self.hours_pending(rx.poi_scratch) || !rx.poi_scratch.detail_valid {
            rx.t(Msg::AssistantVisitUnavailable)
        } else if rx.no_fix {
            rx.t(Msg::AssistantNoFix)
        } else if self
            .poi
            .metadata
            .approach
            .map_or(self.is_landmark(), |a| a.profile_mask & (1 << rx.settings.bike_type as u8) == 0)
        {
            rx.t(Msg::AssistantNoRoad)
        } else {
            use crate::navigator::VisitUnavailable::*;
            match self.visit_error {
                Some(NoFix) => rx.t(Msg::AssistantNoFix),
                Some(NoMappedAccess) => rx.t(Msg::AssistantNoRoad),
                Some(Profile) => rx.t(Msg::AssistantProfileBlocked),
                Some(SourceChanged) => rx.t(Msg::AssistantMapChanged),
                Some(Busy) => rx.t(Msg::AssistantBusy),
                Some(Avoidance) => rx.t(Msg::AssistantBlockedRoute),
                Some(Unmatched) => rx.t(Msg::AssistantUnmatched),
                None => rx.t(Msg::AssistantReviewVisit),
            }
        };
        super::route_overview::draw_start_button(cv, w, h, label);
    }
}

/// The distance rows' pitch; each row's rule sits just above the next row.
const ROW_PITCH: i32 = 40;
/// The cap centre of a ledger row's value, below the row top: the caps bottom out at `y + 32`.
const VALUE_MID: i32 = 32 - Font::Display.cap_height() as i32 / 2;
/// The gap between a row's value and the mark left of it.
const MARK_GAP: i32 = 6;
/// The bearing arrow's radius, at Body-line size like the list rows' arrow.
const BEARING_R: i32 = Font::Body.cap_height() as i32 / 2;

/// One distance ledger row with its rule under it. Returns the value's left edge, for the mark
/// drawn beside it.
fn distance_row(cv: &mut impl Surface, w: i32, y: i32, caption: &str, d_m: u32, units: crate::settings::Units) -> i32 {
    let mut value = heapless::String::<8>::new();
    let unit = write_distance_split(&mut value, d_m, units);
    ledger_row(cv, w, y, caption, &value, unit, None);
    cv.hline(16, y + ROW_PITCH - 2, w - 32, palette::RULE);
    ledger_value_left(w, &value, unit)
}

/// The gap between the "Today" caption and a badge that has no interval rows beside it.
const BADGE_GAP: i32 = 8;

/// The "Today" caption's y and the first interval row's y, under distance rows ending at `rows_end`.
fn hours_top(rows_end: i32) -> (i32, i32) {
    let head_y = rows_end + 16;
    (head_y, head_y + Font::Label.cap_bottom() as i32 + 8)
}

/// The interval rows' face and pitch: Body rows, or the compact Label rows.
fn range_face(compact: bool) -> (Font, i32) {
    if compact {
        (Font::Label, Font::Label.cap_height() as i32 + 2)
    } else {
        (Font::Body, Font::Body.line_height() as i32)
    }
}

/// The bottom of the hours block under distance rows ending at `rows_end`: the last interval row,
/// else the badge of height `badge_h`, else the caption.
fn hours_bottom(rows_end: i32, intervals: usize, compact: bool, badge_h: Option<i32>) -> i32 {
    let (head_y, row_y) = hours_top(rows_end);
    let (font, step) = range_face(compact);
    match (intervals, badge_h) {
        (0, Some(badge_h)) => row_y + BADGE_GAP + badge_h,
        (0, None) => head_y + Font::Label.cap_bottom() as i32,
        (n, _) => row_y + (n as i32 - 1) * step + font.cap_bottom() as i32,
    }
}

/// How far the Today-line badge lifts above the cap-centre of the caption. Centred, the pill
/// crowds the first hours row below it.
const BADGE_RAISE: i32 = 5;

/// The badge's horizontal padding, from the pill edge to the measured text width.
const BADGE_PAD_X: i32 = 8;
/// The badge's vertical padding, from the pill edge to the measured text extents.
const BADGE_PAD_Y: i32 = 8;
/// Format quarter-hours from midnight (`0..=96`, `96` = 24:00) as `HH:MM` into `s`.
fn write_quarter<const N: usize>(s: &mut heapless::String<N>, q: u8) {
    let minutes = q as u16 * 15;
    let _ = write!(s, "{:02}:{:02}", minutes / 60, minutes % 60);
}

/// Write one interval as `HH:MM-HH:MM` into `s`. The dash is an ASCII hyphen, because the bitmap
/// font has no en-dash glyph.
fn write_interval<const N: usize>(s: &mut heapless::String<N>, iv: &Interval) {
    write_quarter(s, iv.open_q);
    let _ = s.push('-');
    write_quarter(s, iv.close_q);
}

/// Draw `text` in [`Font::Body`], wrapped to at most two lines at `max_w` px, and return the y
/// below the last line. A POI name is at most 24 bytes, so it never needs a third line.
fn draw_wrapped(cv: &mut impl Surface, text: &str, x: i32, top: i32, max_w: i32, color: u16) -> i32 {
    let cw = Font::Body.char_width() as i32;
    let max_chars = (max_w / cw).max(1) as usize;
    let line_h = Font::Body.line_height() as i32;
    if text.chars().count() <= max_chars {
        cv.text(text, Point::new(x, top), Font::Body, TextAlign::Left, color);
        return top + Font::Body.cap_bottom() as i32;
    }
    let split = split_at(text, max_chars);
    let (first, rest) = text.split_at(split);
    cv.text(first.trim_end(), Point::new(x, top), Font::Body, TextAlign::Left, color);
    let second = fit_chars(rest.trim_start(), max_chars);
    let y2 = top + line_h;
    cv.text(&second, Point::new(x, y2), Font::Body, TextAlign::Left, color);
    y2 + Font::Body.cap_bottom() as i32
}

/// Byte index to split `text` for a first line of at most `max_chars` chars: the last space at or
/// before `max_chars`, else a hard cut at a char boundary.
fn split_at(text: &str, max_chars: usize) -> usize {
    let mut last_space: Option<usize> = None;
    for (n, (byte_idx, ch)) in text.char_indices().enumerate() {
        if n >= max_chars {
            return last_space.map(|i| i + 1).unwrap_or(byte_idx);
        }
        if ch == ' ' {
            last_space = Some(byte_idx);
        }
    }
    text.len()
}

/// Copy at most `max` chars of `s` into a bounded string.
fn fit_chars(s: &str, max: usize) -> heapless::String<24> {
    let mut out = heapless::String::new();
    for ch in s.chars().take(max) {
        let _ = out.push(ch);
    }
    out
}

fn hours_view(schedule: Option<&WeeklySchedule>, local: Option<(u8, u16)>) -> (Msg, heapless::Vec<Interval, 3>) {
    let Some(schedule) = schedule.filter(|s| s.status(local) != obc_reader::hours::OpeningStatus::Unknown) else {
        return (Msg::PoiDetailHoursNotListed, heapless::Vec::new());
    };
    let intervals = schedule.intervals_on_day(local.expect("known local time").0);
    let heading = if intervals.is_empty() { Msg::PoiDetailClosedToday } else { Msg::PoiDetailToday };
    (heading, intervals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DateTime;
    use obc_formats::obcm::POI_HOURS_BLOB_LEN;
    use obc_reader::weekday_from_ymd;
    use obc_reader::WeeklySchedule;

    /// A pool blob from `flags` and per-day `(open_q, close_q)` slot pairs, Mon..Sun.
    fn blob(flags: u8, days: [[(u8, u8); 2]; 7]) -> [u8; POI_HOURS_BLOB_LEN] {
        let mut b = [0u8; POI_HOURS_BLOB_LEN];
        b[0] = flags;
        let mut i = 1;
        for day in &days {
            for &(o, c) in day {
                b[i] = o;
                b[i + 1] = c;
                i += 2;
            }
        }
        b
    }

    fn sched(days: [[(u8, u8); 2]; 7]) -> WeeklySchedule {
        WeeklySchedule::decode(&blob(0, days)).unwrap()
    }

    /// A DateTime on a known weekday: Mon 2025-01-06, Sun 2025-01-05.
    fn dt(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> DateTime {
        DateTime { year, month, day, hour, minute }
    }

    /// The heading and range strings the draw renders for `sched` on the weekday of `now`. It uses
    /// the production hours view, so the format is asserted without a framebuffer.
    fn render_hours(
        schedule: Option<&WeeklySchedule>,
        now: DateTime,
    ) -> (&'static str, heapless::Vec<heapless::String<16>, 3>) {
        let weekday = weekday_from_ymd(now.year, now.month, now.day);
        let (heading, intervals) =
            hours_view(schedule, Some((weekday, u16::from(now.hour) * 60 + u16::from(now.minute))));
        let head = match heading {
            Msg::PoiDetailHoursNotListed => "Hours not listed",
            Msg::PoiDetailClosedToday => "Closed today",
            _ => "Today",
        };
        let mut rows: heapless::Vec<heapless::String<16>, 3> = heapless::Vec::new();
        for iv in &intervals {
            let mut r: heapless::String<16> = heapless::String::new();
            write_interval(&mut r, iv);
            let _ = rows.push(r);
        }
        (head, rows)
    }

    fn rows_of(rows: &heapless::Vec<heapless::String<16>, 3>) -> heapless::Vec<&str, 3> {
        rows.iter().map(|r| r.as_str()).collect()
    }

    #[test]
    fn trusted_day_display_includes_overnight_spillover() {
        let mut days = [[(0, 0); 2]; 7];
        days[6][0] = (88, 8);
        let schedule = sched(days);
        let (heading, ranges) = hours_view(Some(&schedule), None);
        assert!(matches!(heading, Msg::PoiDetailHoursNotListed));
        assert!(ranges.is_empty(), "an unknown local day cannot claim closed today");
        for minute in [60, 300] {
            let (heading, ranges) = hours_view(Some(&schedule), Some((0, minute)));
            assert!(matches!(heading, Msg::PoiDetailToday));
            assert_eq!(ranges.as_slice(), &[Interval { open_q: 0, close_q: 8 }]);
        }
        assert_eq!(schedule.status(Some((0, 60))), obc_reader::hours::OpeningStatus::Open);
        days[0] = [(32, 48), (56, 72)];
        let (_, ranges) = hours_view(Some(&sched(days)), Some((0, 60)));
        assert_eq!(
            ranges.as_slice(),
            &[
                Interval { open_q: 0, close_q: 8 },
                Interval { open_q: 32, close_q: 48 },
                Interval { open_q: 56, close_q: 72 }
            ]
        );
    }

    #[test]
    fn today_hours_single_interval() {
        // Mon 08:00-18:00 (32,72); render on Monday 2025-01-06.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (32, 72);
        let (head, rows) = render_hours(Some(&sched(days)), dt(2025, 1, 6, 12, 0));
        assert_eq!(head, "Today");
        assert_eq!(rows_of(&rows).as_slice(), &["08:00-18:00"]);
    }

    #[test]
    fn today_hours_two_intervals_split_lunch() {
        // Mon 08:00-12:00, 14:00-18:00 → two stacked range rows.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0] = [(32, 48), (56, 72)];
        let (head, rows) = render_hours(Some(&sched(days)), dt(2025, 1, 6, 10, 0)); // Monday
        assert_eq!(head, "Today");
        assert_eq!(rows_of(&rows).as_slice(), &["08:00-12:00", "14:00-18:00"]);
    }

    #[test]
    fn today_hours_closed_today() {
        // Open Mon only; render on Sunday 2025-01-05 → closed today, no range rows.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (32, 72);
        let (head, rows) = render_hours(Some(&sched(days)), dt(2025, 1, 5, 12, 0)); // Sunday
        assert_eq!(head, "Closed today");
        assert!(rows.is_empty());
    }

    #[test]
    fn no_hours_shows_hours_not_listed() {
        let (head, rows) = render_hours(None, dt(2025, 1, 6, 12, 0));
        assert_eq!(head, "Hours not listed");
        assert!(rows.is_empty());
    }

    #[test]
    fn twenty_four_hour_day_formats_to_2400() {
        // A 24h day (0,96) shows 00:00–24:00.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (0, 96);
        let (head, rows) = render_hours(Some(&sched(days)), dt(2025, 1, 6, 3, 0)); // Monday
        assert_eq!(head, "Today");
        assert_eq!(rows_of(&rows).as_slice(), &["00:00-24:00"]);
    }

    #[test]
    fn open_now_badge_state_from_clock() {
        // Mon 08:00-18:00.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (32, 72);
        let s = sched(days);
        let mon_noon = dt(2025, 1, 6, 12, 0);
        let mon_wd = weekday_from_ymd(mon_noon.year, mon_noon.month, mon_noon.day);
        assert_eq!(mon_wd, 0, "2025-01-06 is Monday");
        assert!(s.is_open(mon_wd, mon_noon.hour as u16 * 60 + mon_noon.minute as u16), "open at Mon noon");

        let mon_night = dt(2025, 1, 6, 23, 30);
        assert!(
            !s.is_open(mon_wd, mon_night.hour as u16 * 60 + mon_night.minute as u16),
            "closed at Mon 23:30 (after hours)"
        );

        let sun_noon = dt(2025, 1, 5, 12, 0);
        let sun_wd = weekday_from_ymd(sun_noon.year, sun_noon.month, sun_noon.day);
        assert_eq!(sun_wd, 6, "2025-01-05 is Sunday");
        assert!(!s.is_open(sun_wd, sun_noon.hour as u16 * 60 + sun_noon.minute as u16), "closed on Sunday");
    }

    /// Each row caption clears the widest value its row can show, and the mark left of that value.
    #[test]
    fn row_captions_clear_the_value_and_its_mark_in_every_language() {
        // "5279 ft" is as wide as any figure the 10 km Ahead range or the 50 km nearby radius can
        // reach. An off-route place is inside the 300 m corridor, so its widest figure is "984 ft".
        let rows = [
            (Msg::PoiDetailAhead, "5279", "ft", 0),
            (Msg::PoiDetailAway, "5279", "ft", MARK_GAP + 2 * BEARING_R),
            (Msg::PoiDetailOffRoute, "984", "ft", MARK_GAP + ARROW_W),
        ];
        for language in crate::settings::Language::ALL {
            for (msg, value, unit, mark_w) in rows {
                let caption = crate::i18n::t(msg, language);
                let caption_right = 16 + text_width(caption, Font::Label) as i32;
                let mark_left = ledger_value_left(240, value, unit) - mark_w;
                assert!(caption_right + MARK_GAP <= mark_left, "{language:?}: {caption}");
            }
        }
    }

    #[test]
    fn quarter_hour_formatting_boundaries() {
        let mut s: heapless::String<8> = heapless::String::new();
        write_quarter(&mut s, 0);
        assert_eq!(s.as_str(), "00:00");
        s.clear();
        write_quarter(&mut s, 34); // 34*15 = 510 min = 08:30
        assert_eq!(s.as_str(), "08:30");
        s.clear();
        write_quarter(&mut s, 96); // 24:00
        assert_eq!(s.as_str(), "24:00");
    }
}
