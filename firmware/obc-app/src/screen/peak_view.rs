//! Peak View: a heading-relative relief panorama with restrained summit labels,
//! and a selected-peak ledger in Browse. Each observer has fixed panorama framing.
//!
//! The draw order is compass, terrain, peak annotations, then the selected-peak ledger.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{rect, text::Font, text::TextAlign, Surface};

use crate::input::Gesture;
use crate::peak_view::{runtime::Status, PeakViewProfile};
use crate::Msg;

use super::vocab::spinner::Spinner;
use super::{palette, vocab::fmt::distance_short, vocab::marquee::fit, Ctx, Render, Transition};

const FULL_Q4: i32 = 360 * 4;
const HALF_Q4: i32 = FULL_Q4 / 2;
const COMPASS_H: i32 = 34;
const LEDGER_H: i32 = 64;
const LABEL_GAP: i32 = 12;
mod terrain;

/// Live mode follows [`crate::AppState::effective_heading_deg`]. Stepping selects a summit and
/// enters Browse, which freezes the panorama so every selection continues to refer to the terrain
/// the rider was looking at. Select enters Browse, then opens installed content or returns to Live.
/// Back leaves Browse before it leaves the screen.
#[derive(Debug, Default)]
pub struct PeakViewScreen {
    browse_heading_q4: Option<u16>,
    pub(crate) browse_position: Option<(i32, i32)>,
    pub(crate) map_generation: Option<u32>,
    selected: Option<obc_formats::obcm::SourceId>,
    spin: Spinner,
    status: Status,
}

impl PeakViewScreen {
    pub fn new(fix: Option<obc_ports::Fix>) -> Self {
        Self { status: if fix.is_some() { Status::Building(0) } else { Status::Waiting }, ..Self::default() }
    }

    pub fn needs_position(&self) -> bool {
        self.status == Status::Waiting
    }

    pub fn set_status(&mut self, status: Status) -> bool {
        let changed = self.status != status;
        if matches!(status, Status::Waiting | Status::Unavailable) {
            self.browse_heading_q4 = None;
            self.browse_position = None;
            self.selected = None;
        }
        self.status = status;
        changed
    }

    pub(crate) fn invalidate_map(&mut self) {
        self.browse_heading_q4 = None;
        self.browse_position = None;
        self.selected = None;
    }

    fn selected_index(&self, profile: &PeakViewProfile) -> Option<usize> {
        self.selected.and_then(|source| profile.peaks.iter().position(|peak| peak.source == source))
    }

    fn select(&mut self, profile: &PeakViewProfile, index: Option<usize>) {
        self.selected = index.map(|i| profile.peaks[i].source);
    }

    pub(crate) fn selected_source(&self) -> Option<obc_formats::obcm::SourceId> {
        self.browse_heading_q4.and(self.selected)
    }

    pub fn heading_q4(&self, state: &crate::AppState) -> u16 {
        self.browse_heading_q4.unwrap_or_else(|| {
            state.peak_view_profile.as_ref().map(|profile| live_heading_q4(state, profile)).unwrap_or(0)
        })
    }

    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32) -> super::ScreenTick {
        if matches!(self.status, Status::Waiting | Status::Unavailable) {
            self.spin.tick_at_cadence(now_ms, w, h, 166)
        } else {
            super::ScreenTick::idle()
        }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if matches!(self.status, Status::Waiting | Status::Unavailable) {
            return if matches!(g, Gesture::Back) { Transition::Pop } else { Transition::None };
        }
        let Some(ref profile) = current_profile(cx.state) else {
            return if matches!(g, Gesture::Back) { Transition::Pop } else { Transition::None };
        };
        match g {
            Gesture::Step(n) => {
                if n == 0 {
                    return Transition::None;
                }
                let mut heading = self.heading_q4(cx.state);
                if self.browse_heading_q4.is_none() {
                    self.selected = None;
                }
                for _ in 0..n.unsigned_abs() {
                    let visible = visible_indices(profile, heading);
                    let current = self.selected_index(profile).and_then(|i| visible.iter().position(|old| *old == i));
                    let next = if let Some(at) = current {
                        let next = at as i32 + n.signum();
                        (0..visible.len() as i32).contains(&next).then(|| visible[next as usize])
                    } else {
                        if n > 0 { visible.first() } else { visible.last() }.copied()
                    };
                    if next.is_some() {
                        self.select(profile, next);
                    } else {
                        heading = normalize_q4(i32::from(heading) + 60 * n.signum()) as u16;
                        let entered = visible_indices(profile, heading);
                        let mut new = entered.iter().copied().filter(|i| !visible.contains(i));
                        let next = if n > 0 { new.next() } else { new.next_back() };
                        self.select(
                            profile,
                            next.or_else(|| self.selected_index(profile).filter(|i| entered.contains(i))),
                        );
                    }
                }
                self.browse_position = Some((profile.observer_lat, profile.observer_lon));
                self.browse_heading_q4 = Some(heading);
                Transition::None
            }
            Gesture::Press => {
                if self.selected_source().is_some_and(|source| cx.landmarks.peak_source == Some(source))
                    && cx.landmarks.ready()
                    && cx.landmarks.article.is_some()
                {
                    cx.landmarks.page = 0;
                    cx.landmarks.reading = true;
                    return Transition::Push(super::Screen::PeakArticle(super::PeakArticleScreen {
                        selection: cx.landmarks.peak.unwrap(),
                        source: self.selected.unwrap(),
                    }));
                }
                if self.browse_heading_q4.take().is_none() {
                    let heading = live_heading_q4(cx.state, profile);
                    self.select(
                        profile,
                        visible_indices(profile, heading).into_iter().max_by_key(|i| profile.peaks[*i].score),
                    );
                    self.browse_position = Some((profile.observer_lat, profile.observer_lon));
                    self.browse_heading_q4 = Some(heading);
                } else {
                    self.selected = None;
                    self.browse_position = None;
                }
                Transition::None
            }
            Gesture::Back if self.browse_heading_q4.take().is_some() => {
                self.selected = None;
                self.browse_position = None;
                Transition::None
            }
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        cv.clear(palette::PARCHMENT);
        if matches!(self.status, Status::Waiting | Status::Unavailable) {
            super::vocab::chrome::title_frame(cv, rx.w, rx.h, rx.t(Msg::MenuPeaks), "");
            self.spin.draw_needle(cv, rx.w, rx.h);
            cv.text(
                rx.t(if self.status == Status::Waiting { Msg::PeakViewWaitingGps } else { Msg::PeakViewUnavailable }),
                Point::new(rx.w / 2, rx.h / 2 + 65),
                Font::Label,
                TextAlign::Center,
                palette::SUBTEXT,
            );
            return;
        }
        let Some(ref profile) = current_profile(rx.state) else {
            cv.text(
                rx.t(Msg::PeakViewNoPeaks),
                Point::new(rx.w / 2, rx.h / 2 - 12),
                Font::Label,
                TextAlign::Center,
                palette::SUBTEXT,
            );
            return;
        };

        let heading_q4 = self.browse_heading_q4.unwrap_or_else(|| live_heading_q4(rx.state, profile));
        let selected = self
            .browse_heading_q4
            .and_then(|_| self.selected_index(profile))
            .filter(|i| peak_is_visible(profile, *i, heading_q4));

        let mode = self.browse_heading_q4.map(|_| rx.t(Msg::PeakViewManual));
        draw_compass(cv, rx.w, heading_q4, profile.horizontal_fov_q4(), mode);
        if matches!(self.status, Status::Building(_)) {
            for x in [rx.w - 18, rx.w - 13, rx.w - 8] {
                cv.fill(rect(x, COMPASS_H - 8, 2, 2), palette::PARCHMENT);
            }
        }
        let chart_bottom = rx.h - LEDGER_H;
        terrain::draw(cv, rx.peak_view, profile, heading_q4, rx.w, chart_bottom);
        if rx.peak_view.is_some() {
            draw_peak_annotations(cv, profile, heading_q4, selected, rx.w, chart_bottom);
        }
        draw_ledger(cv, rx, profile, selected, heading_q4);
    }
}

fn current_profile(state: &crate::AppState) -> Option<PeakViewProfile<'_>> {
    let mut profile = state.peak_view_profile?;
    if state.peak_view_peak_count > 0 {
        profile.peaks = &state.peak_view_peaks[..state.peak_view_peak_count as usize];
    }
    Some(profile)
}

fn live_heading_q4(state: &crate::AppState, profile: &PeakViewProfile) -> u16 {
    state
        .compass_deg
        .or_else(|| state.effective_heading_deg())
        .map(|deg| normalize_q4((deg * 4.0 + 0.5) as i32) as u16)
        .unwrap_or(profile.default_heading_q4)
}

fn normalize_q4(angle: i32) -> i32 {
    angle.rem_euclid(FULL_Q4)
}

fn bearing_delta_q4(bearing: u16, center: u16) -> i32 {
    (bearing as i32 - center as i32 + HALF_Q4).rem_euclid(FULL_Q4) - HALF_Q4
}

fn bearing_x(bearing: u16, center: u16, w: i32, fov: i32) -> Option<i32> {
    let delta = bearing_delta_q4(bearing, center);
    (delta.abs() <= fov / 2).then_some((delta + fov / 2) * (w - 1) / fov)
}

fn visible_indices(profile: &PeakViewProfile, heading: u16) -> heapless::Vec<usize, 64> {
    let mut indices: heapless::Vec<_, 64> =
        (0..profile.peaks.len()).filter(|i| peak_is_visible(profile, *i, heading)).collect();
    indices.sort_unstable_by_key(|i| {
        let peak = &profile.peaks[*i];
        (bearing_delta_q4(peak.azimuth_q4, heading), peak.distance_m, peak.lat, peak.lon)
    });
    indices
}

fn peak_is_visible(profile: &PeakViewProfile, index: usize, heading_q4: u16) -> bool {
    profile.peaks.get(index).is_some_and(|peak| {
        peak.visible && bearing_delta_q4(peak.azimuth_q4, heading_q4).abs() <= profile.horizontal_fov_q4() / 2
    })
}

fn draw_compass(cv: &mut impl Surface, w: i32, heading_q4: u16, fov: i32, mode: Option<&str>) {
    cv.fill(rect(0, 0, w, COMPASS_H), palette::WOOD);
    for bearing_deg in (0..360).step_by(15) {
        let bearing_q4 = (bearing_deg * 4) as u16;
        let Some(x) = bearing_x(bearing_q4, heading_q4, w, fov) else { continue };
        let cardinal = match bearing_deg {
            0 => Some("N"),
            90 => Some("E"),
            180 => Some("S"),
            270 => Some("W"),
            _ => None,
        };
        if let Some(label) = cardinal {
            cv.text(label, Point::new(x, 2), Font::Label, TextAlign::Center, palette::PARCHMENT);
        } else {
            cv.vline(x, 22, 7, 1, palette::WOOD_LIGHT);
        }
    }
    let mut heading: heapless::String<8> = heapless::String::new();
    let _ = write!(heading, "{:03}°", (heading_q4 as u32 + 2) / 4 % 360);
    cv.fill(rect(w / 2 - 27, 0, 54, 24), palette::AMBER);
    cv.text(&heading, Point::new(w / 2, 1), Font::Label, TextAlign::Center, palette::INK);
    cv.triangle(
        Point::new(w / 2, COMPASS_H - 1),
        Point::new(w / 2 - 5, COMPASS_H - 8),
        Point::new(w / 2 + 5, COMPASS_H - 8),
        palette::AMBER,
    );
    if let Some(mode) = mode {
        cv.fill(rect(0, 0, w / 2 - 28, 24), palette::WOOD);
        cv.text(mode, Point::new(4, 1), Font::Label, TextAlign::Left, palette::PARCHMENT);
    }
}

fn angle_y(profile: &PeakViewProfile, angle_q4: i16, bottom: i32) -> i32 {
    let height = bottom - COMPASS_H;
    let (angle_bottom, angle_top) = profile.vertical_bounds_q4();
    let span = angle_top - angle_bottom;
    let above_bottom = i32::from(angle_q4) - angle_bottom;
    (bottom - above_bottom * height / span).clamp(COMPASS_H, bottom - 1)
}

/// Show every name that fits. Apparent elevation breaks collisions; selection does not rearrange labels.
fn annotation_indices(profile: &PeakViewProfile, heading: u16, w: i32) -> heapless::Vec<usize, 64> {
    let mut indices = visible_indices(profile, heading);
    indices.sort_unstable_by_key(|i| {
        let peak = &profile.peaks[*i];
        (core::cmp::Reverse(peak.score), peak.distance_m, peak.lat, peak.lon)
    });
    let x = |i: usize| {
        bearing_x(profile.peaks[i].azimuth_q4, heading, w, profile.horizontal_fov_q4())
            .unwrap_or(0)
            .clamp(6, w.max(12) - 6)
    };
    let mut count = 0;
    for at in 0..indices.len() {
        let i = indices[at];
        if indices[..count].iter().any(|old| (x(i) - x(*old)).abs() < 15) {
            continue;
        }
        indices[count] = i;
        count += 1;
    }
    indices.truncate(count);
    indices
}

fn draw_peak_annotations(
    cv: &mut impl Surface,
    profile: &PeakViewProfile,
    heading_q4: u16,
    selected: Option<usize>,
    w: i32,
    bottom: i32,
) {
    for i in annotation_indices(profile, heading_q4, w) {
        let peak = &profile.peaks[i];
        let x = bearing_x(peak.azimuth_q4, heading_q4, w, profile.horizontal_fov_q4()).unwrap_or(0);
        let summit_y = angle_y(profile, peak.angle_q4, bottom);
        let headroom = (summit_y - LABEL_GAP - COMPASS_H - 2).max(12);
        // The label is drawn at divisor 2, so its cells are half a Label cell wide.
        let name = fit(peak.name.as_str(), headroom * 2, Font::Label);
        let label_bottom = (summit_y - LABEL_GAP).max(COMPASS_H + 14);
        let color = if Some(i) == selected { palette::WOOD } else { palette::INK };
        let leader_top = (summit_y - LABEL_GAP + 1).max(COMPASS_H + 2);
        cv.vline(x, leader_top, (summit_y - leader_top).max(0), 1, color);
        cv.text_ccw(&name, Point::new((x - 6).clamp(0, (w - 12).max(0)), label_bottom), Font::Label, 2, color);
    }

    if let Some(i) = selected {
        let peak = &profile.peaks[i];
        let anchor = peak.azimuth_q4;
        if let Some(x) = bearing_x(anchor, heading_q4, w, profile.horizontal_fov_q4()) {
            let y = angle_y(profile, peak.angle_q4, bottom);
            cv.triangle(Point::new(x, y + 1), Point::new(x - 7, y - 11), Point::new(x + 7, y - 11), palette::INK);
            cv.triangle(Point::new(x, y - 1), Point::new(x - 5, y - 9), Point::new(x + 5, y - 9), palette::AMBER);
        }
    }
}

fn draw_ledger(cv: &mut impl Surface, rx: &Render, profile: &PeakViewProfile, selected: Option<usize>, heading: u16) {
    let top = rx.h - LEDGER_H;
    cv.fill(rect(0, top, rx.w, LEDGER_H), palette::PARCHMENT);
    cv.hline(0, top, rx.w, palette::WOOD);
    let ledger_budget = rx.w - 20;
    let Some(peak) = selected.and_then(|i| profile.peaks.get(i)) else {
        let pending = rx.peak_view.is_none_or(|terrain| !terrain.view_ready(heading, profile.horizontal_fov_q4()));
        if !pending && !visible_indices(profile, heading).is_empty() {
            return;
        }
        let status =
            fit(rx.t(if pending { Msg::PeakViewPreparing } else { Msg::PeakViewNoPeaks }), ledger_budget, Font::Label);
        cv.text(&status, Point::new(10, top + 7), Font::Label, TextAlign::Left, palette::SUBTEXT);
        return;
    };

    let info = rx.landmarks.peak_source == Some(peak.source) && rx.landmarks.ready() && rx.landmarks.article.is_some();
    // The info disc takes two cells off the name.
    let name_budget = ledger_budget - if info { 2 * Font::Label.char_width() as i32 } else { 0 };
    if info {
        cv.disc(Point::new(rx.w - 16, top + 17), 9, palette::WOOD);
        cv.text("i", Point::new(rx.w - 16, top + 5), Font::Label, TextAlign::Center, palette::PARCHMENT);
    }
    let name_row = rect(10, top + 5, rx.w - if info { 44 } else { 20 }, Font::Label.line_height() as i32);
    let name = rx.marquee.fit(peak.name.as_str(), name_budget, Font::Label, Some(name_row));
    cv.text(&name, Point::new(10, top + 5), Font::Label, TextAlign::Left, palette::INK);
    let mut details: heapless::String<40> = heapless::String::new();
    if let Some(meters) = peak.elevation_m {
        let elevation = libm::roundf(rx.settings.units.elev(meters as f32)) as i32;
        let _ = write!(details, "{}{}  ", elevation, rx.settings.units.elev_label());
    }
    let distance = distance_short(peak.distance_m, rx.settings.units);
    let _ = write!(details, "{}  {}", distance, cardinal(peak.azimuth_q4));
    cv.text(&details, Point::new(10, top + 34), Font::Label, TextAlign::Left, palette::SUBTEXT);
}

fn cardinal(azimuth_q4: u16) -> &'static str {
    const CARDINALS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    CARDINALS[((azimuth_q4 as usize + 90) / 180) % CARDINALS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::peak_view::{PeakName, PeakViewPeak};
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};

    static PEAKS: [PeakViewPeak; 3] = [
        PeakViewPeak {
            source: obc_formats::obcm::SourceId(1),
            name: PeakName::new("A"),
            lat: 0,
            lon: 0,
            elevation_m: Some(1000),
            distance_m: 1000,
            azimuth_q4: 112,
            angle_q4: 4,
            visible: true,
            score: 1,
        },
        PeakViewPeak {
            source: obc_formats::obcm::SourceId(2),
            name: PeakName::new("C"),
            lat: 1,
            lon: 0,
            elevation_m: Some(1500),
            distance_m: 3000,
            azimuth_q4: 720,
            angle_q4: 4,
            visible: true,
            score: 2,
        },
        PeakViewPeak {
            source: obc_formats::obcm::SourceId(3),
            name: PeakName::new("B"),
            lat: 2,
            lon: 0,
            elevation_m: Some(2000),
            distance_m: 2000,
            azimuth_q4: 1328,
            angle_q4: 4,
            visible: true,
            score: 3,
        },
    ];
    // A wide window keeps the peaks selectable from the headings used below.
    static PROFILE: PeakViewProfile<'static> = PeakViewProfile {
        observer_lat: 0,
        observer_lon: 0,
        observer_elevation_m: 0,
        default_heading_q4: 0,
        fov_q4: 467,
        vertical_centre_q4: 80,
        vertical_span_q4: 345,
        peaks: &PEAKS,
    };
    static STACKED_PEAKS: [PeakViewPeak; 3] = [
        PeakViewPeak {
            source: obc_formats::obcm::SourceId(4),
            name: PeakName::new("Near"),
            lat: 3,
            lon: 0,
            elevation_m: Some(1000),
            distance_m: 1000,
            azimuth_q4: 120,
            angle_q4: 4,
            visible: true,
            score: 1,
        },
        PeakViewPeak {
            source: obc_formats::obcm::SourceId(5),
            name: PeakName::new("Middle"),
            lat: 4,
            lon: 0,
            elevation_m: Some(2000),
            distance_m: 4000,
            azimuth_q4: 120,
            angle_q4: 8,
            visible: true,
            score: 2,
        },
        PeakViewPeak {
            source: obc_formats::obcm::SourceId(6),
            name: PeakName::new("Far"),
            lat: 5,
            lon: 0,
            elevation_m: Some(3000),
            distance_m: 8000,
            azimuth_q4: 120,
            angle_q4: 12,
            visible: true,
            score: 3,
        },
    ];
    static STACKED_PROFILE: PeakViewProfile<'static> = PeakViewProfile {
        observer_lat: 0,
        observer_lon: 0,
        observer_elevation_m: 0,
        default_heading_q4: 120,
        fov_q4: 38,
        vertical_centre_q4: 6,
        vertical_span_q4: 28,
        peaks: &STACKED_PEAKS,
    };

    #[test]
    fn opening_uses_an_available_fix_before_the_runtime_starts() {
        for has_fix in [false, true] {
            let mut app = crate::App::new(AppState::new(0, 0, 1.0));
            app.state.peak_view_profile = Some(PROFILE);
            let mut loc = crate::harness::support::OnceFix(has_fix.then_some(obc_ports::Fix::at(0, 0)));
            app.tick(obc_ports::RideClock(0), obc_ports::Sensors::new(&mut loc), None);
            assert!(app.show_peak_view());
            let super::super::Screen::PeakView(screen) = app.top_screen() else { panic!("Peak View") };
            assert_eq!(screen.status, if has_fix { Status::Building(0) } else { Status::Waiting });
        }
    }

    #[test]
    fn visible_order_crosses_north_and_includes_stacked_crests() {
        assert_eq!(bearing_delta_q4(40, 1400), 80);
        assert_eq!(&visible_indices(&PROFILE, 0)[..], &[2, 0]);
        assert_eq!(&visible_indices(&STACKED_PROFILE, 120)[..], &[0, 1, 2]);
        let hidden = [PEAKS[0], PeakViewPeak { visible: false, ..PEAKS[2] }];
        assert_eq!(&visible_indices(&PeakViewProfile { peaks: &hidden, ..PROFILE }, 0)[..], &[0]);
    }

    #[test]
    fn annotations_fill_available_space_and_rank_only_collisions() {
        let mut peaks = [PEAKS[0]; 16];
        for (i, peak) in peaks.iter_mut().enumerate() {
            peak.lat = i as i32;
            peak.azimuth_q4 = 20 * i as u16;
            peak.score = 100 - i as u32;
        }
        // Ten high-ranked candidates collide. The six lower-ranked names still have room.
        for peak in &mut peaks[..10] {
            peak.azimuth_q4 = 0;
        }
        let profile = PeakViewProfile { fov_q4: 320, peaks: &peaks, ..PROFILE };
        let labels = annotation_indices(&profile, 160, 256);
        assert_eq!(&labels[..], &[0, 10, 11, 12, 13, 14, 15]);
        let reordered: std::vec::Vec<_> = peaks.iter().rev().copied().collect();
        let reordered = PeakViewProfile { peaks: &reordered, ..profile };
        let selected: std::vec::Vec<_> =
            annotation_indices(&reordered, 160, 256).into_iter().map(|i| reordered.peaks[i].lat).collect();
        assert_eq!(selected, [0, 10, 11, 12, 13, 14, 15]);
    }

    #[test]
    fn browse_selects_by_prominence_pans_and_survives_refill() {
        let mut state = AppState::new(0, 0, 1.0);
        state.peak_view_profile = Some(PROFILE);
        state.compass_deg = Some(0.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut screen = PeakViewScreen::new(None);
        screen.set_status(Status::Building(0));
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);
        assert_eq!(screen.selected, None);
        screen.handle(Gesture::Press, &mut cx);
        assert_eq!(screen.selected, Some(PEAKS[2].source));
        screen.handle(Gesture::Press, &mut cx);
        assert_eq!(screen.selected, None);
        screen.handle(Gesture::Step(1), &mut cx);
        assert_eq!(screen.selected, Some(PEAKS[2].source));
        screen.handle(Gesture::Step(1), &mut cx);
        assert_eq!(screen.selected, Some(PEAKS[0].source));
        assert_eq!(screen.browse_heading_q4, Some(0));
        cx.state.peak_view_peaks[0] = PEAKS[2];
        cx.state.peak_view_peaks[1] = PEAKS[0];
        cx.state.peak_view_peak_count = 2;
        assert_eq!(screen.selected_index(&current_profile(cx.state).unwrap()), Some(1));
        screen.handle(Gesture::Step(1), &mut cx);
        assert_eq!(screen.browse_heading_q4, Some(60));
        assert_eq!(screen.selected, Some(PEAKS[0].source));
        screen.handle(Gesture::Step(2), &mut cx);
        assert_eq!(screen.browse_heading_q4, Some(180));
        screen.handle(Gesture::Press, &mut cx);
        assert_eq!(screen.heading_q4(cx.state), 0);
        screen.handle(Gesture::Step(-1), &mut cx);
        assert_eq!(screen.selected, Some(PEAKS[0].source));
        screen.handle(Gesture::Step(-1), &mut cx);
        assert_eq!(screen.selected, Some(PEAKS[2].source));
    }

    #[test]
    fn browse_visits_every_peak_across_empty_space_and_reverses_without_skips() {
        let mut state = AppState::new(0, 0, 1.0);
        state.peak_view_profile = Some(PeakViewProfile { fov_q4: 240, ..PROFILE });
        state.compass_deg = Some(0.0);
        // Two peaks enter together at 40 degrees; two more share the same bearing.
        for (i, bearing) in [340, 0, 40, 44, 110, 110, 180, 280].into_iter().enumerate() {
            state.peak_view_peaks[i] = PeakViewPeak {
                source: obc_formats::obcm::SourceId(i as u64),
                lat: i as i32,
                azimuth_q4: bearing * 4,
                distance_m: 1000 + i as u32,
                ..PEAKS[0]
            };
        }
        state.peak_view_peak_count = 8;
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);
        for (direction, expected) in [(1, [0, 1, 2, 3, 4, 5, 6, 7, 0]), (-1, [1, 0, 7, 6, 5, 4, 3, 2, 1])] {
            let mut screen = PeakViewScreen::new(None);
            screen.set_status(Status::Ready);
            let mut visited = std::vec::Vec::new();
            for _ in 0..40 {
                let heading = screen.heading_q4(cx.state);
                screen.handle(Gesture::Step(direction), &mut cx);
                let turn = bearing_delta_q4(screen.heading_q4(cx.state), heading);
                assert!(turn == 0 || turn == direction * 60);
                if let Some(obc_formats::obcm::SourceId(id)) = screen.selected {
                    if visited.last() != Some(&id) {
                        visited.push(id);
                    }
                }
                if visited.len() == expected.len() {
                    break;
                }
            }
            assert_eq!(visited, expected);
        }

        let mut screen = PeakViewScreen::new(None);
        screen.set_status(Status::Ready);
        screen.handle(Gesture::Step(2), &mut cx);
        assert_eq!(screen.selected, Some(obc_formats::obcm::SourceId(1)));
        screen.handle(Gesture::Step(1), &mut cx);
        assert_eq!(screen.browse_heading_q4, Some(60));
        assert_eq!(screen.selected, Some(obc_formats::obcm::SourceId(2)));
        screen.handle(Gesture::Step(1), &mut cx);
        assert_eq!(screen.selected, Some(obc_formats::obcm::SourceId(3)));
        screen.handle(Gesture::Step(1), &mut cx);
        assert_eq!(screen.browse_heading_q4, Some(120));
        assert_eq!(screen.selected, Some(obc_formats::obcm::SourceId(3)));
        screen.handle(Gesture::Step(-1), &mut cx);
        assert_eq!(screen.selected, Some(obc_formats::obcm::SourceId(2)));
        screen.handle(Gesture::Step(4), &mut cx);
        assert_eq!(screen.selected, None);
        screen.handle(Gesture::Step(-1), &mut cx);
        assert_eq!(screen.selected, Some(obc_formats::obcm::SourceId(3)));
    }

    #[test]
    fn vertical_labels_keep_utf8_and_fit_even_the_smallest_headroom() {
        assert_eq!(fit("Grossglockner", 6 * 12, Font::Label).as_str(), "Gros..");
        assert_eq!(fit("Älplerhorn", 4 * 12, Font::Label).as_str(), "Äl..");
        assert_eq!(fit("Peak", 2 * 12, Font::Label).as_str(), "..", "the smallest headroom keeps the dots alone");
        assert_eq!(fit("Peak", 4 * 12, Font::Label).as_str(), "Peak");
    }

    #[test]
    fn only_waiting_and_unavailable_animate() {
        let mut screen = PeakViewScreen::new(None);
        screen.tick_timers(0, 240, 320);
        assert!(screen.tick_timers(166, 240, 320).changed);
        assert!(screen.set_status(Status::Building(0)));
        assert!(!screen.set_status(Status::Building(0)));
        assert!(screen.tick_timers(200, 240, 320).next_wake_ms.is_none());
    }
}
