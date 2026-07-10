//! The POI **detail** screen — reached by pressing a POI in the [list](super::PoiListScreen),
//! carrying the selected [`Poi`](obc_reader::Poi). Shows the **full stored name** un-ellipsized
//! (the list row ellipsizes to fit), the subtype label as a muted subtitle, the same live
//! [bearing arrow](super::poi_list) the list draws, **today's opening hours**, and an
//! **OPEN / CLOSED-now** badge (epic #439 P4 #444).
//!
//! # Hours read at draw (the reader-in-draw seam)
//!
//! [`Reader::poi_hours`](obc_reader::Reader::poi_hours) resolves the POI's pooled weekly schedule
//! (spec §7.5), and the [`Reader`](obc_reader::Reader) lives **only** in the draw context
//! ([`Render::reader`]). So — exactly like the list's lazy snapshot (#425) — the schedule is read
//! **once**, on the first draw that has a `Reader`, into a [`Cell`]-held cache on the screen (a
//! `WeeklySchedule` is a ~29-byte `Copy` value). The tri-state cache distinguishes *not resolved
//! yet* (`None`) from *resolved to no hours* (`Some(None)`) from *resolved to a schedule*
//! (`Some(Some(_))`), so a POI with no `hours_ref` is read at most once too, never per frame.
//! [`base_needs_reader`](crate::App::base_needs_reader) keeps `rx.reader` `Some` until that first
//! read lands, then the board host stops rebuilding the reader per frame — the same energy
//! discipline as the list snapshot. The draw stays `&self`; the cache mutates through the `Cell`.
//!
//! The **open-now** badge reads the live local wall-clock ([`Render::now`]) each frame: today's
//! weekday + minute-of-day feed [`WeeklySchedule::is_open`]. By the time a POI detail is on screen
//! the device already has a fix (the list required one), so the local date is plausible — no
//! separate "clock unset" state in v1 (see the epic's locked decision).

use core::cell::Cell;
use core::fmt::Write;

use embedded_graphics::prelude::Point;
use obc_reader::{label_of, weekday_from_ymd, Interval, Poi, WeeklySchedule};
use obc_render::{
    rect,
    text::{Font, TextAlign},
    Surface,
};

use crate::input::Gesture;
use crate::Msg;

use super::poi_list::draw_bearing_arrow;
use super::{palette, title_frame, Ctx, Render, Screen, Transition, LIST_TOP};

/// The POI detail. Carries the selected [`Poi`] (name / coords / subtype / `hours_ref`) plus a
/// lazily-resolved schedule cache. The `Poi` widens the [`Screen`](super::Screen) enum by its size;
/// the cache is a small `Copy` value behind a `Cell` (see the module docs on the reader-in-draw
/// seam).
#[derive(Debug)]
pub struct PoiDetailScreen {
    poi: Poi,
    /// The resolved schedule, cached on the first draw with a `Reader`. Tri-state: `None` = not
    /// resolved yet (keep asking for the reader), `Some(None)` = resolved to *no hours*
    /// (`hours_ref` 0xFFFF or an out-of-range ref), `Some(Some(_))` = the pooled schedule. `Cell`
    /// so the one draw-time read mutates without a `&mut self` draw.
    schedule: Cell<Option<Option<WeeklySchedule>>>,
}

impl PoiDetailScreen {
    /// Open the detail for `poi` (cloned out of the list snapshot by the list's `Gesture::Press`).
    /// The schedule is resolved lazily on the first draw with a `Reader`.
    pub fn new(poi: Poi) -> Self {
        PoiDetailScreen { poi, schedule: Cell::new(None) }
    }

    /// Whether the schedule cache still needs a `Reader` at draw — it hasn't resolved yet. Drives
    /// [`base_needs_reader`](crate::App::base_needs_reader) so the board host keeps building the
    /// reader until the one hours read lands, then stops.
    pub(crate) fn hours_pending(&self) -> bool {
        self.schedule.get().is_none()
    }

    pub fn handle(&mut self, g: Gesture, _cx: &mut Ctx) -> Transition {
        match g {
            // Create a route to this POI (epic #116, R4): press opens the "Create a route?"
            // confirm. The route's name is the POI's stored name, or its subtype fallback label —
            // the same fallback the list row shows, so the catalog entry reads like the row did.
            Gesture::Press => {
                let name = if self.poi.name.is_empty() {
                    label_of(self.poi.subtype).unwrap_or("POI")
                } else {
                    self.poi.name.as_str()
                };
                Transition::Push(Screen::NavConfirm(super::NavConfirmScreen::new(
                    (self.poi.lon, self.poi.lat),
                    name,
                    obc_reader::category_of(self.poi.subtype),
                )))
            }
            Gesture::Back => Transition::Pop, // return to the POI list
            _ => Transition::None,
        }
    }

    /// Resolve the POI's schedule on the first draw that has a `Reader`, caching it in `self.schedule`
    /// through the `Cell`. A no-op once resolved (the cache is `Some`). Runs in the draw path — the
    /// only place [`Render::reader`] exists — so [`base_needs_reader`](crate::App::base_needs_reader)
    /// keeps `rx.reader` `Some` here until this lands.
    fn ensure_schedule(&self, rx: &Render) {
        if self.schedule.get().is_some() {
            return; // already resolved (possibly to `None` — no hours)
        }
        let Some(reader) = rx.reader else {
            return; // no map this frame — retry next draw
        };
        self.schedule.set(Some(reader.poi_hours(self.poi.hours_ref)));
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        use palette::*;
        self.ensure_schedule(rx);

        let (w, h) = (rx.w, rx.h);
        title_frame(cv, w, h, rx.t(Msg::PoiDetailTitle), "");

        // The subtype fallback label ("Supermarket", "Pharmacy", …) — the subtitle, and the whole
        // name line when the POI is unnamed.
        let label = label_of(self.poi.subtype).unwrap_or("POI");
        let named = !self.poi.name.is_empty();
        let name = if named { self.poi.name.as_str() } else { label };

        // Name row — the category's pixel icon (the poi_menu glyphs, drawn unscaled: their ~22 px
        // box is a Body line's height) at the left inset, the name beside it (#685 §2). The name
        // stays un-ellipsized: Body-tier, wrapping to a second line rather than truncating; a
        // wrapped second line runs under the icon's column, which reads fine (the icon marks the
        // row, not a margin).
        let x = 16;
        let name_top = LIST_TOP + 4;
        let mut name_x = x;
        if let Some(cat) = obc_reader::category_of(self.poi.subtype) {
            let icon_c = Point::new(x + 11, name_top + Font::Body.cap_height() as i32 / 2);
            super::poi_menu::draw_category_icon(cv, cat, icon_c, INK, PARCHMENT);
            name_x = x + 22 + 8;
        }
        let name_bot = draw_wrapped(cv, name, name_x, name_top, w - name_x - 16, INK);

        // Subtitle — the subtype label, muted, under the name. Skipped when the name line already IS
        // the label (unnamed POI), so it never repeats.
        let mut sub_bot = name_bot;
        if named {
            let sub_y = name_bot + 6;
            cv.text(label, Point::new(x, sub_y), Font::Label, TextAlign::Left, SUBTEXT);
            sub_bot = sub_y + Font::Label.cap_height() as i32;
        }

        // Distance + bearing row — promoted directly under the category line (#685 §2: the two
        // numbers that decide "do I go"). The same 8-way arrow as the list rows at Body-line size,
        // then the distance in Body type (`1km`; metres below 1 km — the list's format). The arrow
        // hides when there's no heading reference (GPS course while moving / compass while
        // stopped, the #231 seam); the distance stays.
        let dist_y = sub_bot + 14;
        let heading = rx.state.effective_heading_deg();
        let fix = rx.state.user_fix;
        let arrow_r = Font::Body.cap_height() as i32 / 2;
        let mut dist_x = x;
        if let (Some(fix), Some(heading)) = (fix, heading) {
            let arrow_mid = dist_y + arrow_r;
            draw_bearing_arrow(
                cv,
                Point::new(x + arrow_r, arrow_mid),
                arrow_r,
                (fix.lon, fix.lat),
                (self.poi.lon, self.poi.lat),
                heading,
            );
            dist_x = x + 2 * arrow_r + 8;
        }
        let mut dist: heapless::String<12> = heapless::String::new();
        super::write_off_route(&mut dist, "", self.poi.distance_m, rx.settings.units);
        cv.text(&dist, Point::new(dist_x, dist_y), Font::Body, TextAlign::Left, INK);

        // Today's hours — a muted heading row ("Today" / "Closed today" / "Hours not listed"), then
        // each open interval on its own Body row (`08:00 – 18:00`). Stacking the (up to two) ranges
        // keeps each within the 240 px panel, where a single two-range line wouldn't fit.
        let head_y = dist_y + Font::Body.cap_height() as i32 + 16;
        let schedule = self.schedule.get().flatten();
        let weekday = weekday_from_ymd(rx.now.year, rx.now.month, rx.now.day);
        let intervals: &[Interval] = match &schedule {
            Some(sched) => sched.today_intervals(weekday),
            None => &[],
        };
        let head = match schedule {
            None => rx.t(Msg::PoiDetailHoursNotListed),
            Some(_) if intervals.is_empty() => rx.t(Msg::PoiDetailClosedToday),
            Some(_) => rx.t(Msg::PoiDetailToday),
        };
        cv.text(head, Point::new(x, head_y), Font::Label, TextAlign::Left, SUBTEXT);

        let mut row_y = head_y + Font::Label.cap_height() as i32 + 8;
        for iv in intervals {
            let mut range: heapless::String<16> = heapless::String::new();
            write_interval(&mut range, iv);
            cv.text(&range, Point::new(x, row_y), Font::Body, TextAlign::Left, INK);
            row_y += Font::Body.cap_height() as i32 + 6;
        }

        // OPEN / CLOSED-now badge — only when the POI has a schedule; read from the live wall-clock
        // this frame. A rounded pill (#685 §2: 3 px radius, 18 px tall, 8 px horizontal padding),
        // Label type on white — green fill when open, warning-red when closed, so the closed state
        // reads as a state, not just quieter text.
        if let Some(sched) = schedule {
            let minute = rx.now.hour as u16 * 60 + rx.now.minute as u16;
            let open = sched.is_open(weekday, minute);
            let (text, bg) = if open { (rx.t(Msg::PoiDetailOpen), ON) } else { (rx.t(Msg::PoiDetailClosed), WARNING) };
            let badge_y = row_y + 8;
            let badge_w = text.chars().count() as i32 * Font::Label.char_width() as i32 + 16;
            let badge_h = 18;
            cv.round(rect(x, badge_y, badge_w, badge_h), 3, bg);
            cv.text_vcentered(text, x + badge_w / 2, (badge_y, badge_h), Font::Label, TextAlign::Center, PARCHMENT);
        }

        // Footer action row — `▶Route here`, exactly the Route overview's START RIDE bar (#685 §2:
        // the shared drawer, so the two can't drift). Press anywhere already opened the create-route
        // confirm; the bar only makes that visible. Back still returns to the list.
        super::route_overview::draw_start_button(cv, w, h, rx.t(Msg::PoiDetailRouteHere));
    }
}

/// Format quarter-hours from midnight (`0..=96`, `96` = 24:00) as `HH:MM` into `s`.
fn write_quarter<const N: usize>(s: &mut heapless::String<N>, q: u8) {
    let minutes = q as u16 * 15;
    let _ = write!(s, "{:02}:{:02}", minutes / 60, minutes % 60);
}

/// Write one interval as `HH:MM-HH:MM` into `s`. A plain ASCII hyphen — the Terminus bitmap font
/// has no en-dash glyph (it renders as a `?`), so the range dash is a hyphen throughout.
fn write_interval<const N: usize>(s: &mut heapless::String<N>, iv: &Interval) {
    write_quarter(s, iv.open_q);
    let _ = s.push('-');
    write_quarter(s, iv.close_q);
}

/// Draw `text` in [`Font::Body`], wrapping to a second line on a word boundary when it overflows
/// `max_w` px (a 24-byte POI name almost always fits one line; a long one gets two rather than being
/// clipped). Returns the y just below the last line drawn. At most two lines — a POI name never
/// needs a third.
fn draw_wrapped(cv: &mut impl Surface, text: &str, x: i32, top: i32, max_w: i32, color: u16) -> i32 {
    let cw = Font::Body.char_width() as i32;
    let max_chars = (max_w / cw).max(1) as usize;
    let line_h = Font::Body.cap_height() as i32 + 6;
    if text.chars().count() <= max_chars {
        cv.text(text, Point::new(x, top), Font::Body, TextAlign::Left, color);
        return top + Font::Body.cap_height() as i32;
    }
    // Split into two lines on the last space that keeps the first line within `max_chars`; fall back
    // to a hard char split if there's no such space (one very long token).
    let split = split_at(text, max_chars);
    let (first, rest) = text.split_at(split);
    cv.text(first.trim_end(), Point::new(x, top), Font::Body, TextAlign::Left, color);
    // Second line: truncate to fit; the name is at most 24 bytes, so two lines always cover it.
    let second = fit_chars(rest.trim_start(), max_chars);
    let y2 = top + line_h;
    cv.text(&second, Point::new(x, y2), Font::Body, TextAlign::Left, color);
    y2 + Font::Body.cap_height() as i32
}

/// Byte index to split `text` for a first line of at most `max_chars` chars — the last space at or
/// before `max_chars`, else a hard char-boundary cut at `max_chars` (no space to break on).
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

/// Copy at most `max` chars of `s` into a bounded string (char-boundary safe). Used for the wrapped
/// name's second line, which a 24-byte name never overruns.
fn fit_chars(s: &str, max: usize) -> heapless::String<24> {
    let mut out = heapless::String::new();
    for ch in s.chars().take(max) {
        let _ = out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DateTime;
    use obc_reader::POI_HOURS_BLOB_LEN;

    /// A 29-byte pool blob from `flags` + per-day `(open_q, close_q)` slot pairs (Mon..Sun) — the
    /// same shape the reader/packer hours tests build.
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

    /// A DateTime on a known weekday: 2025-01-01 is a Wednesday (weekday index 2), so pick concrete
    /// dates for the tests. Mon 2025-01-06, Sun 2025-01-05.
    fn dt(year: u16, month: u8, day: u8, hour: u8, minute: u8) -> DateTime {
        DateTime { year, month, day, hour, minute }
    }

    /// The heading + per-interval range strings the draw would render for `sched` on `now`'s
    /// weekday — mirrors the draw's `head`/`intervals` selection so the format is asserted without a
    /// framebuffer (this crate is `no_std`, so a `heapless::Vec` collects the rows). `schedule`
    /// `None` = the POI has no hours at all.
    fn hours_view(
        schedule: Option<&WeeklySchedule>,
        now: DateTime,
    ) -> (&'static str, heapless::Vec<heapless::String<16>, 2>) {
        let weekday = weekday_from_ymd(now.year, now.month, now.day);
        let intervals: &[Interval] = schedule.map(|s| s.today_intervals(weekday)).unwrap_or(&[]);
        let head = match schedule {
            None => "Hours not listed",
            Some(_) if intervals.is_empty() => "Closed today",
            Some(_) => "Today",
        };
        let mut rows: heapless::Vec<heapless::String<16>, 2> = heapless::Vec::new();
        for iv in intervals {
            let mut r: heapless::String<16> = heapless::String::new();
            write_interval(&mut r, iv);
            let _ = rows.push(r);
        }
        (head, rows)
    }

    /// The range strings from a [`hours_view`] result, as `&str`s for comparison.
    fn rows_of(rows: &heapless::Vec<heapless::String<16>, 2>) -> heapless::Vec<&str, 2> {
        rows.iter().map(|r| r.as_str()).collect()
    }

    #[test]
    fn today_hours_single_interval() {
        // Mon 08:00-18:00 (32,72); render on Monday 2025-01-06.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (32, 72);
        let (head, rows) = hours_view(Some(&sched(days)), dt(2025, 1, 6, 12, 0));
        assert_eq!(head, "Today");
        assert_eq!(rows_of(&rows).as_slice(), &["08:00-18:00"]);
    }

    #[test]
    fn today_hours_two_intervals_split_lunch() {
        // Mon 08:00-12:00, 14:00-18:00 → two stacked range rows.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0] = [(32, 48), (56, 72)];
        let (head, rows) = hours_view(Some(&sched(days)), dt(2025, 1, 6, 10, 0)); // Monday
        assert_eq!(head, "Today");
        assert_eq!(rows_of(&rows).as_slice(), &["08:00-12:00", "14:00-18:00"]);
    }

    #[test]
    fn today_hours_closed_today() {
        // Open Mon only; render on Sunday 2025-01-05 → closed today, no range rows.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (32, 72);
        let (head, rows) = hours_view(Some(&sched(days)), dt(2025, 1, 5, 12, 0)); // Sunday
        assert_eq!(head, "Closed today");
        assert!(rows.is_empty());
    }

    #[test]
    fn no_hours_shows_hours_not_listed() {
        let (head, rows) = hours_view(None, dt(2025, 1, 6, 12, 0));
        assert_eq!(head, "Hours not listed");
        assert!(rows.is_empty());
    }

    #[test]
    fn twenty_four_hour_day_formats_to_2400() {
        // A 24h day (0,96) shows 00:00–24:00.
        let mut days = [[(0u8, 0u8); 2]; 7];
        days[0][0] = (0, 96);
        let (head, rows) = hours_view(Some(&sched(days)), dt(2025, 1, 6, 3, 0)); // Monday
        assert_eq!(head, "Today");
        assert_eq!(rows_of(&rows).as_slice(), &["00:00-24:00"]);
    }

    #[test]
    fn open_now_badge_state_from_clock() {
        // Mon 08:00-18:00; is_open on the same weekday/minute the badge computes.
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
