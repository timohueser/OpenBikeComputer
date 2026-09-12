//! Peak View: a heading-relative relief panorama with restrained summit labels,
//! and one permanent selected-peak ledger. The horizontal window follows each profile's vertical
//! span (see [`fov_q4`]) with mild vertical exaggeration.
//!
//! The draw order is compass, terrain, peak annotations, then the selected-peak ledger.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{rect, text::Font, text::TextAlign, Surface};

use crate::input::Gesture;
use crate::peak_view::PeakViewProfile;
use crate::Msg;

use super::vocab::spinner::Spinner;
use super::{palette, vocab::fmt::distance_short, Ctx, Render, Transition};

const FULL_Q4: i32 = 360 * 4;
const HALF_Q4: i32 = FULL_Q4 / 2;
const COMPASS_H: i32 = 34;
const LEDGER_H: i32 = 64;
const LABEL_GAP: i32 = 12;
mod terrain;

/// Live mode follows [`crate::AppState::effective_heading_deg`]. Stepping selects a summit and
/// enters Browse, which freezes the panorama so every selection continues to refer to the terrain
/// the rider was looking at. Select toggles Live/Browse; Back leaves Browse before it leaves the
/// screen.
#[derive(Debug, Default)]
pub struct PeakViewScreen {
    browse_heading_q4: Option<u16>,
    selected: u8,
    spin: Spinner,
    loading: bool,
    failed: bool,
    waiting: bool,
    building: bool,
}

impl PeakViewScreen {
    pub fn new() -> Self {
        Self { loading: true, ..Self::default() }
    }

    pub fn set_loading(&mut self, loading: bool, failed: bool) -> bool {
        let changed = self.loading != loading || self.failed != failed || self.waiting;
        self.waiting = false;
        self.loading = loading;
        self.failed = failed;
        if loading {
            self.browse_heading_q4 = None;
        }
        changed
    }

    pub fn set_waiting(&mut self) -> bool {
        let changed = !self.waiting;
        self.waiting = true;
        self.loading = false;
        self.failed = false;
        changed
    }

    pub fn set_building(&mut self, building: bool) -> bool {
        let changed = self.building != building;
        self.building = building;
        changed
    }

    pub fn heading_q4(&self, state: &crate::AppState) -> u16 {
        self.browse_heading_q4.unwrap_or_else(|| {
            state.peak_view_profile.as_ref().map(|profile| live_heading_q4(state, profile)).unwrap_or(0)
        })
    }

    pub fn tick_timers(&mut self, now_ms: u32, w: i32, h: i32) -> super::ScreenTick {
        if self.loading || self.waiting {
            self.spin.tick_at_cadence(now_ms, w, h, 166)
        } else {
            super::ScreenTick::idle()
        }
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        if self.loading || self.failed || self.waiting {
            return if matches!(g, Gesture::Back) { Transition::Pop } else { Transition::None };
        }
        let Some(ref profile) = current_profile(cx.state) else {
            return if matches!(g, Gesture::Back) { Transition::Pop } else { Transition::None };
        };
        match g {
            Gesture::Step(n) => {
                let heading = self.browse_heading_q4.unwrap_or_else(|| live_heading_q4(cx.state, profile));
                let current = self
                    .browse_heading_q4
                    .map(|_| self.selected as usize)
                    .or_else(|| nearest_visible_peak(profile, heading));
                let Some(next) = stepped_visible_peak(profile, heading, current, n) else {
                    return Transition::None;
                };
                self.selected = next as u8;
                self.browse_heading_q4 = Some(heading);
                Transition::None
            }
            Gesture::Press => {
                if self.browse_heading_q4.take().is_none() {
                    let heading = live_heading_q4(cx.state, profile);
                    if let Some(selected) = nearest_visible_peak(profile, heading) {
                        self.selected = selected as u8;
                        self.browse_heading_q4 = Some(heading);
                    }
                }
                Transition::None
            }
            Gesture::Back if self.browse_heading_q4.take().is_some() => Transition::None,
            Gesture::Back => Transition::Pop,
            Gesture::Hold | Gesture::BackHold => Transition::None,
        }
    }

    pub fn draw(&self, cv: &mut impl Surface, rx: &mut Render) {
        cv.clear(palette::PARCHMENT);
        if self.loading || self.failed || self.waiting {
            super::vocab::chrome::title_frame(cv, rx.w, rx.h, rx.t(Msg::MenuPeaks), "");
            if self.loading || self.waiting {
                self.spin.draw_needle(cv, rx.w, rx.h);
            }
            cv.text(
                rx.t(if self.waiting {
                    Msg::PeakViewWaitingGps
                } else if self.failed {
                    Msg::PeakViewUnavailable
                } else {
                    Msg::PeakViewPreparing
                }),
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
        let selected = if self.browse_heading_q4.is_some() {
            profile.peaks.get(self.selected as usize).map(|_| self.selected as usize)
        } else {
            nearest_visible_peak(profile, heading_q4)
        };

        let mode = self.browse_heading_q4.map(|_| rx.t(Msg::PeakViewManual));
        draw_compass(cv, rx.w, heading_q4, fov_q4(profile), mode);
        if self.building {
            for x in [rx.w - 18, rx.w - 13, rx.w - 8] {
                cv.fill(rect(x, COMPASS_H - 8, 2, 2), palette::PARCHMENT);
            }
        }
        let chart_bottom = rx.h - LEDGER_H;
        if let Some(terrain) = rx.peak_view {
            terrain::draw(cv, terrain, profile, heading_q4, rx.w, chart_bottom);
        }
        draw_peak_annotations(cv, profile, heading_q4, selected, rx.w, chart_bottom);
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
        .effective_heading_deg()
        .map(|deg| normalize_q4((deg * 4.0 + 0.5) as i32) as u16)
        .unwrap_or(profile.default_heading_q4)
}

/// Wide relief gets a wider window; distant relief gets a closer view.
fn fov_q4(profile: &PeakViewProfile) -> i32 {
    profile.horizontal_fov_q4()
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

fn nearest_visible_peak(profile: &PeakViewProfile, heading_q4: u16) -> Option<usize> {
    profile
        .peaks
        .iter()
        .enumerate()
        .filter(|(index, _)| peak_is_visible(profile, *index, heading_q4))
        .min_by_key(|(index, peak)| (bearing_delta_q4(peak.azimuth_q4, heading_q4).abs(), peak.distance_m, *index))
        .map(|(index, _)| index)
}

/// Select by left-to-right ridge order without allocating a second peak list. Named summits on
/// exposed near, middle, and far crests inside the frozen panorama all participate.
fn stepped_visible_peak(
    profile: &PeakViewProfile,
    heading_q4: u16,
    current: Option<usize>,
    steps: i32,
) -> Option<usize> {
    let mut current = current
        .filter(|index| peak_is_visible(profile, *index, heading_q4))
        .or_else(|| nearest_visible_peak(profile, heading_q4))?;
    for _ in 0..steps.unsigned_abs() {
        current = adjacent_visible_peak(profile, heading_q4, current, steps.is_positive())?;
    }
    Some(current)
}

fn peak_order_key(profile: &PeakViewProfile, index: usize, heading_q4: u16) -> Option<(i32, u32, usize)> {
    let peak = profile.peaks.get(index)?;
    Some((bearing_delta_q4(peak.azimuth_q4, heading_q4), peak.distance_m, index))
}

fn adjacent_visible_peak(profile: &PeakViewProfile, heading_q4: u16, current: usize, forward: bool) -> Option<usize> {
    let current_key = peak_order_key(profile, current, heading_q4)?;
    let candidates = profile.peaks.iter().enumerate().filter(|(index, _)| peak_is_visible(profile, *index, heading_q4));
    if forward {
        candidates
            .clone()
            .filter(|(index, _)| peak_order_key(profile, *index, heading_q4).is_some_and(|key| key > current_key))
            .min_by_key(|(index, _)| peak_order_key(profile, *index, heading_q4))
            .or_else(|| candidates.min_by_key(|(index, _)| peak_order_key(profile, *index, heading_q4)))
            .map(|(index, _)| index)
    } else {
        candidates
            .clone()
            .filter(|(index, _)| peak_order_key(profile, *index, heading_q4).is_some_and(|key| key < current_key))
            .max_by_key(|(index, _)| peak_order_key(profile, *index, heading_q4))
            .or_else(|| candidates.max_by_key(|(index, _)| peak_order_key(profile, *index, heading_q4)))
            .map(|(index, _)| index)
    }
}

fn peak_is_visible(profile: &PeakViewProfile, index: usize, heading_q4: u16) -> bool {
    profile
        .peaks
        .get(index)
        .is_some_and(|peak| peak.visible && bearing_delta_q4(peak.azimuth_q4, heading_q4).abs() <= fov_q4(profile) / 2)
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

fn draw_peak_annotations(
    cv: &mut impl Surface,
    profile: &PeakViewProfile,
    heading_q4: u16,
    selected: Option<usize>,
    w: i32,
    bottom: i32,
) {
    // Keep the ten strongest visible candidates, then take the first five that do not collide.
    // This base set never depends on selection: choosing Matterhorn may recolor its own label, but
    // it must not free a slot and make an unrelated name suddenly appear.
    let mut ranked: [Option<(usize, u32)>; 10] = [None; 10];
    for (i, peak) in profile.peaks.iter().enumerate() {
        if !peak_is_visible(profile, i, heading_q4) {
            continue;
        }
        for slot in 0..ranked.len() {
            if ranked[slot].is_none_or(|(_, score)| peak.score > score) {
                for move_to in (slot + 1..ranked.len()).rev() {
                    ranked[move_to] = ranked[move_to - 1];
                }
                ranked[slot] = Some((i, peak.score));
                break;
            }
        }
    }

    let mut label_x = [i32::MIN; 5];
    let mut labels = 0;
    for candidate in ranked.into_iter().flatten() {
        if labels == label_x.len() {
            break;
        }
        let peak = &profile.peaks[candidate.0];
        let anchor = peak.azimuth_q4;
        let x = bearing_x(anchor, heading_q4, w, fov_q4(profile)).unwrap_or(0);
        if label_x[..labels].iter().any(|old| (x - *old).abs() < 15) {
            continue;
        }
        let summit_y = angle_y(profile, peak.angle_q4, bottom);
        let run_h = peak.name.as_str().chars().count() as i32 * 6;
        if summit_y - LABEL_GAP - run_h >= COMPASS_H + 2 {
            let color = if Some(candidate.0) == selected { palette::WOOD } else { palette::INK };
            cv.vline(x, summit_y - LABEL_GAP + 1, LABEL_GAP - 1, 1, color);
            cv.text_ccw(peak.name.as_str(), Point::new(x - 6, summit_y - LABEL_GAP), Font::Label, 2, color);
            label_x[labels] = x;
            labels += 1;
        }
    }

    if let Some(i) = selected {
        let peak = &profile.peaks[i];
        let anchor = peak.azimuth_q4;
        if let Some(x) = bearing_x(anchor, heading_q4, w, fov_q4(profile)) {
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
    let Some(peak) = selected.and_then(|i| profile.peaks.get(i)) else {
        let pending = rx.peak_view.is_some_and(|terrain| !terrain.view_ready(heading, fov_q4(profile)));
        let mut caption = heapless::String::new();
        let status = super::vocab::tiles::fit_caption(
            rx.t(if pending { Msg::PeakViewPreparing } else { Msg::PeakViewNoPeaks }),
            rx.w - 20,
            &mut caption,
            Font::Label,
        );
        cv.text(status, Point::new(10, top + 7), Font::Label, TextAlign::Left, palette::SUBTEXT);
        return;
    };

    let mut caption = heapless::String::new();
    let name = super::vocab::tiles::fit_caption(peak.name.as_str(), rx.w - 20, &mut caption, Font::Label);
    cv.text(name, Point::new(10, top + 5), Font::Label, TextAlign::Left, palette::INK);
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
            name: PeakName::new("C"),
            lat: 0,
            lon: 0,
            elevation_m: Some(1500),
            distance_m: 3000,
            azimuth_q4: 720,
            angle_q4: 4,
            visible: true,
            score: 2,
        },
        PeakViewPeak {
            name: PeakName::new("B"),
            lat: 0,
            lon: 0,
            elevation_m: Some(2000),
            distance_m: 2000,
            azimuth_q4: 1328,
            angle_q4: 4,
            visible: true,
            score: 3,
        },
    ];
    // The wide angle range gives this profile a ~104-degree derived window, so the three peaks
    // spread across the circle stay selectable from the headings the tests use.
    static PROFILE: PeakViewProfile<'static> = PeakViewProfile {
        id: 99,
        name: "test",
        observer_lat: 0,
        observer_lon: 0,
        observer_elevation_m: 0,
        default_heading_q4: 0,
        angle_bottom_q4: -40,
        angle_top_q4: 200,
        peaks: &PEAKS,
    };
    static STACKED_PEAKS: [PeakViewPeak; 3] = [
        PeakViewPeak {
            name: PeakName::new("Near"),
            lat: 0,
            lon: 0,
            elevation_m: Some(1000),
            distance_m: 1000,
            azimuth_q4: 120,
            angle_q4: 4,
            visible: true,
            score: 1,
        },
        PeakViewPeak {
            name: PeakName::new("Middle"),
            lat: 0,
            lon: 0,
            elevation_m: Some(2000),
            distance_m: 4000,
            azimuth_q4: 120,
            angle_q4: 8,
            visible: true,
            score: 2,
        },
        PeakViewPeak {
            name: PeakName::new("Far"),
            lat: 0,
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
        id: 100,
        name: "stacked",
        observer_lat: 0,
        observer_lon: 0,
        observer_elevation_m: 0,
        default_heading_q4: 120,
        angle_bottom_q4: -4,
        angle_top_q4: 16,
        peaks: &STACKED_PEAKS,
    };

    #[test]
    fn peak_selection_wraps_across_north() {
        assert_eq!(bearing_delta_q4(40, 1400), 80);
        assert_eq!(nearest_visible_peak(&PROFILE, 0), Some(0));
        assert_eq!(nearest_visible_peak(&PROFILE, 1430), Some(2));
    }

    #[test]
    fn an_occluded_summit_is_neither_selected_nor_stepped_to() {
        static PEAKS_WITH_HIDDEN: [PeakViewPeak; 3] =
            [PEAKS[0], PeakViewPeak { visible: false, azimuth_q4: 0, ..PEAKS[1] }, PEAKS[2]];
        let profile = PeakViewProfile { peaks: &PEAKS_WITH_HIDDEN, ..PROFILE };
        assert!(!peak_is_visible(&profile, 1, 0));
        assert_eq!(nearest_visible_peak(&profile, 0), Some(0));
        assert_eq!(stepped_visible_peak(&profile, 0, Some(0), 1), Some(2));
    }

    #[test]
    fn stacked_near_middle_and_far_crests_are_independently_selectable() {
        assert!((0..3).all(|index| peak_is_visible(&STACKED_PROFILE, index, 120)));
        assert_eq!(nearest_visible_peak(&STACKED_PROFILE, 120), Some(0));
        assert_eq!(stepped_visible_peak(&STACKED_PROFILE, 120, Some(0), 1), Some(1));
        assert_eq!(stepped_visible_peak(&STACKED_PROFILE, 120, Some(1), 1), Some(2));
    }

    #[test]
    fn browse_freezes_the_profile_and_steps_only_through_its_visible_peaks() {
        let mut state = AppState::new(0, 0, 1.0);
        state.peak_view_profile = Some(PROFILE);
        state.compass_deg = Some(0.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut screen = PeakViewScreen::new();
        screen.set_loading(false, false);
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);

        assert!(matches!(screen.handle(Gesture::Step(1), &mut cx), Transition::None));
        assert_eq!(screen.browse_heading_q4, Some(0), "Browse must not recenter the terrain on the selected peak");
        assert_eq!(screen.selected, 2, "the next summit is B on the left edge; C is outside this profile");

        screen.handle(Gesture::Step(1), &mut cx);
        assert_eq!(screen.browse_heading_q4, Some(0));
        assert_eq!(screen.selected, 0, "selection wraps among the peaks visible in the frozen profile");
    }
    #[test]
    fn loading_animates_cancels_and_stops_waking_when_ready() {
        let mut screen = PeakViewScreen::new();
        let mut state = AppState::new(0, 0, 1.0);
        state.peak_view_profile = Some(PROFILE);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);
        screen.tick_timers(0, 240, 320);
        assert!(!screen.tick_timers(100, 240, 320).changed);
        let tick = screen.tick_timers(166, 240, 320);
        assert!(tick.changed && tick.next_wake_ms.is_some());
        assert!(matches!(screen.handle(Gesture::Step(1), &mut cx), Transition::None));
        assert!(screen.browse_heading_q4.is_none());
        assert!(matches!(screen.handle(Gesture::Back, &mut cx), Transition::Pop));
        screen.set_loading(false, false);
        assert!(screen.set_building(true));
        assert!(!screen.set_building(true), "background progress does not request another redraw");
        assert!(screen.tick_timers(200, 240, 320).next_wake_ms.is_none());
        screen.set_loading(false, true);
        assert!(matches!(screen.handle(Gesture::Back, &mut cx), Transition::Pop));
    }
}
