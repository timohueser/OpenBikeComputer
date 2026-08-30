//! Peak View: a heading-relative panorama with three terrain depths, restrained summit labels,
//! and one permanent selected-peak ledger. The horizontal window follows each profile's vertical
//! span (see [`fov_q4`]) so mountains keep near-true proportions instead of stretching into
//! needles.
//!
//! The draw order is deliberately explicit: compass, terrain, peak annotations, ledger. Sun and
//! route overlays can later become sibling annotation passes without changing the terrain contract
//! or the interaction state. This first slice contains peaks only.

use core::fmt::Write as _;

use embedded_graphics::prelude::Point;
use obc_render::{rect, text::Font, text::TextAlign, Surface};

use crate::input::Gesture;
use crate::peak_view::PeakViewProfile;
use crate::Msg;

use super::{palette, vocab::fmt::distance_short, Ctx, Render, Transition};

const FULL_Q4: i32 = 360 * 4;
const HALF_Q4: i32 = FULL_Q4 / 2;
const COMPASS_H: i32 = 34;
const LEDGER_H: i32 = 64;
/// Accept a summit whose calculated elevation angle reaches its DEM ridge to within two
/// quarter-degrees. The stored ridge is a max over each 4-degree window, so it can legitimately
/// sit a rounding step above the summit's own angle without the summit being hidden.
const RIDGE_TOLERANCE_Q4: i16 = 2;
/// A farther band whose ridge is within one quarter-degree of a nearer band still counts as
/// peeking over it: the sampling cannot distinguish the two, and real summits are sharper than
/// the pooled ridge line.
const EXPOSURE_SLACK_Q4: i16 = 1;

const TERRAIN_NEAR: u16 = palette::rgb565(0, 85, 0); // device-64: dark green
const TERRAIN_MIDDLE: u16 = palette::rgb565(85, 170, 85); // device-64: middle green
const TERRAIN_FAR: u16 = palette::rgb565(170, 255, 170); // device-64: pale green

/// Live mode follows [`crate::AppState::effective_heading_deg`]. Stepping selects a summit and
/// enters Browse, which freezes the panorama so every selection continues to refer to the terrain
/// the rider was looking at. Select toggles Live/Browse; Back leaves Browse before it leaves the
/// screen.
#[derive(Debug, Default)]
pub struct PeakViewScreen {
    browse_heading_q4: Option<u16>,
    selected: u8,
}

impl PeakViewScreen {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn handle(&mut self, g: Gesture, cx: &mut Ctx) -> Transition {
        let Some(profile) = cx.state.peak_view_profile else {
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
        let Some(profile) = rx.state.peak_view_profile else {
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

        draw_compass(cv, rx.w, heading_q4, fov_q4(profile));
        let chart_bottom = rx.h - LEDGER_H;
        draw_terrain(cv, profile, heading_q4, rx.w, chart_bottom);
        draw_peak_annotations(cv, profile, heading_q4, selected, rx.w, chart_bottom);
        draw_ledger(cv, rx, profile, selected, self.browse_heading_q4.is_some());
    }
}

fn live_heading_q4(state: &crate::AppState, profile: &PeakViewProfile) -> u16 {
    state
        .effective_heading_deg()
        .map(|deg| normalize_q4((deg * 4.0 + 0.5) as i32) as u16)
        .unwrap_or(profile.default_heading_q4)
}

/// The horizontal window, derived from the profile's vertical span so vertical exaggeration is
/// a constant 1.8 on the 240-wide, 222-tall panorama chart (`fov = span * 240 * 1.8 / 222`).
/// A big-relief scene such as Kleine Scheidegg gets a wide window; a distant-relief scene such
/// as Gornergrat gets a narrower, zoomed one where a horn still looks like a horn.
fn fov_q4(profile: &PeakViewProfile) -> i32 {
    (profile.angle_top_q4 - profile.angle_bottom_q4).max(1) as i32 * 72 / 37
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
        .filter_map(|(index, _)| {
            let anchor = peak_anchor_q4(profile, index)?;
            let delta = bearing_delta_q4(anchor, heading_q4).abs();
            peak_is_visible(profile, index, heading_q4).then_some((index, delta, profile.peaks[index].layer))
        })
        .min_by_key(|(index, delta, layer)| (*delta, *layer, *index))
        .map(|(index, _, _)| index)
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

fn peak_order_key(profile: &PeakViewProfile, index: usize, heading_q4: u16) -> Option<(i32, u8, usize)> {
    Some((bearing_delta_q4(peak_anchor_q4(profile, index)?, heading_q4), profile.peaks.get(index)?.layer, index))
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

/// Find the crest in the summit's own distance band: the nearest local maximum within 1.5
/// samples, enough to absorb angular downsampling without attaching a name to a distant
/// neighbouring summit. A summit on a monotone stretch of its band's skyline has no local
/// maximum; it anchors at the sample nearest its azimuth instead, and [`peak_reaches_ridge`]
/// still rejects a name that sits below the drawn silhouette.
fn peak_anchor_q4(profile: &PeakViewProfile, index: usize) -> Option<u16> {
    let peak = profile.peaks.get(index)?;
    let layer = *profile.layers_q4.get(peak.layer as usize)?;
    let count = layer.len();
    if count < 3 || profile.sample_step_q4 == 0 {
        return None;
    }
    let search_q4 = profile.sample_step_q4 as i32 * 3 / 2;
    (0..count)
        .filter_map(|sample| {
            let bearing = (sample * profile.sample_step_q4 as usize) as u16;
            let delta = bearing_delta_q4(bearing, peak.azimuth_q4).abs();
            if delta > search_q4 {
                return None;
            }
            let previous = ((sample + count - 1) % count * profile.sample_step_q4 as usize) as u16;
            let next = ((sample + 1) % count * profile.sample_step_q4 as usize) as u16;
            let (left, center, right) = (
                horizon_q4(layer, profile.sample_step_q4, previous),
                horizon_q4(layer, profile.sample_step_q4, bearing),
                horizon_q4(layer, profile.sample_step_q4, next),
            );
            let is_crest = center >= left && center >= right && (center > left || center > right);
            Some((bearing, !is_crest, delta, center))
        })
        .min_by_key(|(_, slope, delta, height)| (*slope, *delta, -*height))
        .map(|(bearing, _, _, _)| bearing)
}

fn peak_ridge_q4(profile: &PeakViewProfile, peak: &crate::PeakViewPeak, bearing_q4: u16) -> Option<i16> {
    let layer = *profile.layers_q4.get(peak.layer as usize)?;
    Some(horizon_q4(layer, profile.sample_step_q4, bearing_q4))
}

fn peak_reaches_ridge(profile: &PeakViewProfile, peak: &crate::PeakViewPeak) -> bool {
    peak_ridge_q4(profile, peak, peak.azimuth_q4).is_some_and(|ridge| peak.angle_q4 + RIDGE_TOLERANCE_Q4 >= ridge)
}

/// A farther ridge line is covered when a nearer band rises clearly above it. Near ridges are
/// always exposed because they are drawn last.
fn peak_ridge_is_exposed(profile: &PeakViewProfile, peak: &crate::PeakViewPeak, anchor_q4: u16) -> bool {
    let layer = peak.layer as usize;
    let Some(ridge) = peak_ridge_q4(profile, peak, anchor_q4) else { return false };
    profile.layers_q4[..layer]
        .iter()
        .all(|nearer| ridge + EXPOSURE_SLACK_Q4 >= horizon_q4(nearer, profile.sample_step_q4, anchor_q4))
}

fn peak_is_visible(profile: &PeakViewProfile, index: usize, heading_q4: u16) -> bool {
    let Some(peak) = profile.peaks.get(index) else { return false };
    let Some(anchor) = peak_anchor_q4(profile, index) else { return false };
    if bearing_delta_q4(anchor, heading_q4).abs() > fov_q4(profile) / 2
        || !peak_reaches_ridge(profile, peak)
        || !peak_ridge_is_exposed(profile, peak, anchor)
    {
        return false;
    }

    // A coarse crest in one distance band may attract several nearby peak nodes. Keep one stable
    // name for that rendered summit, without collapsing stacked near/middle/far ridges that share
    // a bearing.
    !profile.peaks.iter().enumerate().any(|(other_index, other)| {
        if other_index == index
            || other.layer != peak.layer
            || !peak_reaches_ridge(profile, other)
            || peak_anchor_q4(profile, other_index) != Some(anchor)
        {
            return false;
        }
        let distance = bearing_delta_q4(peak.azimuth_q4, anchor).abs();
        let other_distance = bearing_delta_q4(other.azimuth_q4, anchor).abs();
        other.score > peak.score
            || (other.score == peak.score && other_distance < distance)
            || (other.score == peak.score && other_distance == distance && other_index < index)
    })
}

fn draw_compass(cv: &mut impl Surface, w: i32, heading_q4: u16, fov: i32) {
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
}

fn draw_terrain(cv: &mut impl Surface, profile: &PeakViewProfile, heading_q4: u16, w: i32, bottom: i32) {
    let colors = [TERRAIN_NEAR, TERRAIN_MIDDLE, TERRAIN_FAR];
    let fov = fov_q4(profile);
    for layer in (0..3).rev() {
        for x in 0..w {
            let bearing = normalize_q4(heading_q4 as i32 - fov / 2 + x * fov / (w - 1).max(1));
            let angle = horizon_q4(profile.layers_q4[layer], profile.sample_step_q4, bearing as u16);
            let y = angle_y(profile, angle, bottom);
            cv.vline(x, y, bottom - y, 1, colors[layer]);
        }
        trace_horizon(cv, profile, profile.layers_q4[layer], heading_q4, fov, w, bottom, palette::INK);
    }
}

#[allow(clippy::too_many_arguments)]
fn trace_horizon(
    cv: &mut impl Surface,
    profile: &PeakViewProfile,
    samples: &[i16],
    heading_q4: u16,
    fov: i32,
    w: i32,
    bottom: i32,
    color: u16,
) {
    let mut previous = None;
    for x in 0..w {
        let bearing = normalize_q4(heading_q4 as i32 - fov / 2 + x * fov / (w - 1).max(1));
        let angle = horizon_q4(samples, profile.sample_step_q4, bearing as u16);
        let y = angle_y(profile, angle, bottom);
        if let Some((last_x, last_y)) = previous {
            cv.line(Point::new(last_x, last_y), Point::new(x, y), color);
        }
        previous = Some((x, y));
    }
}

fn horizon_q4(samples: &[i16], step_q4: u16, bearing_q4: u16) -> i16 {
    if samples.is_empty() || step_q4 == 0 {
        return 0;
    }
    let whole = bearing_q4 as usize / step_q4 as usize;
    let next = (whole + 1) % samples.len();
    let rem = bearing_q4 as i32 % step_q4 as i32;
    let a = samples[whole % samples.len()] as i32;
    let b = samples[next] as i32;
    (a + (b - a) * rem / step_q4 as i32) as i16
}

fn angle_y(profile: &PeakViewProfile, angle_q4: i16, bottom: i32) -> i32 {
    let height = bottom - COMPASS_H;
    let span = (profile.angle_top_q4 - profile.angle_bottom_q4).max(1) as i32;
    let above_bottom = (angle_q4 - profile.angle_bottom_q4) as i32;
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
        let anchor = peak_anchor_q4(profile, candidate.0).unwrap_or(peak.azimuth_q4);
        let x = bearing_x(anchor, heading_q4, w, fov_q4(profile)).unwrap_or(0);
        if label_x[..labels].iter().any(|old| (x - *old).abs() < 15) {
            continue;
        }
        let summit_y = angle_y(profile, peak_ridge_q4(profile, peak, anchor).unwrap_or(peak.angle_q4), bottom);
        let run_h = peak.name.chars().count() as i32 * 6;
        if summit_y - 4 - run_h >= COMPASS_H + 2 {
            let color = if Some(candidate.0) == selected { palette::WOOD } else { palette::INK };
            cv.vline(x, summit_y - 5, 5, 1, color);
            cv.text_ccw(peak.name, Point::new(x - 6, summit_y - 5), Font::Label, 2, color);
            label_x[labels] = x;
            labels += 1;
        }
    }

    if let Some(i) = selected {
        let peak = &profile.peaks[i];
        let anchor = peak_anchor_q4(profile, i).unwrap_or(peak.azimuth_q4);
        if let Some(x) = bearing_x(anchor, heading_q4, w, fov_q4(profile)) {
            let y = angle_y(profile, peak_ridge_q4(profile, peak, anchor).unwrap_or(peak.angle_q4), bottom);
            cv.triangle(Point::new(x, y - 1), Point::new(x - 5, y - 9), Point::new(x + 5, y - 9), palette::AMBER);
        }
    }
}

fn draw_ledger(cv: &mut impl Surface, rx: &Render, profile: &PeakViewProfile, selected: Option<usize>, manual: bool) {
    let top = rx.h - LEDGER_H;
    cv.fill(rect(0, top, rx.w, LEDGER_H), palette::PARCHMENT);
    cv.hline(0, top, rx.w, palette::WOOD);
    let Some(peak) = selected.and_then(|i| profile.peaks.get(i)) else {
        cv.text(rx.t(Msg::PeakViewNoPeaks), Point::new(10, top + 7), Font::Label, TextAlign::Left, palette::SUBTEXT);
        return;
    };

    cv.text(peak.name, Point::new(10, top + 5), Font::Label, TextAlign::Left, palette::INK);
    let mut details: heapless::String<40> = heapless::String::new();
    let elevation = (rx.settings.units.elev(peak.elevation_m as f32) + 0.5) as u32;
    let distance = distance_short(peak.distance_m, rx.settings.units);
    if manual {
        let _ = write!(details, "{}{}  {}", elevation, rx.settings.units.elev_label(), distance);
    } else {
        let _ = write!(
            details,
            "{}{}  {}  {}",
            elevation,
            rx.settings.units.elev_label(),
            distance,
            cardinal(peak.azimuth_q4)
        );
    }
    cv.text(&details, Point::new(10, top + 34), Font::Label, TextAlign::Left, palette::SUBTEXT);
    if manual {
        cv.text(
            rx.t(Msg::PeakViewManual),
            Point::new(rx.w - 10, top + 34),
            Font::Label,
            TextAlign::Right,
            palette::WOOD,
        );
    }
}

fn cardinal(azimuth_q4: u16) -> &'static str {
    const CARDINALS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
    CARDINALS[((azimuth_q4 as usize + 90) / 180) % CARDINALS.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::{Activity, Mode};
    use crate::peak_view::PeakViewPeak;
    use crate::screen::test_ctx;
    use crate::{AppState, Settings};

    static LAYER: [i16; 12] = [0, 4, 0, 0, 0, 0, 4, 0, 0, 0, 0, 4];
    static INTERPOLATION_LAYER: [i16; 4] = [0, 4, 8, 4];
    static PEAKS: [PeakViewPeak; 3] = [
        PeakViewPeak {
            name: "A",
            elevation_m: 1000,
            distance_m: 1000,
            azimuth_q4: 112,
            angle_q4: 4,
            layer: 0,
            score: 1,
        },
        PeakViewPeak {
            name: "C",
            elevation_m: 1500,
            distance_m: 3000,
            azimuth_q4: 720,
            angle_q4: 4,
            layer: 0,
            score: 2,
        },
        PeakViewPeak {
            name: "B",
            elevation_m: 2000,
            distance_m: 2000,
            azimuth_q4: 1328,
            angle_q4: 4,
            layer: 0,
            score: 3,
        },
    ];
    // The wide angle range gives this profile a ~104-degree derived window, so the three peaks
    // spread across the circle stay selectable from the headings the tests use.
    static PROFILE: PeakViewProfile = PeakViewProfile {
        id: 99,
        name: "test",
        observer_lat: 0,
        observer_lon: 0,
        observer_elevation_m: 0,
        default_heading_q4: 0,
        sample_step_q4: 120,
        angle_bottom_q4: -40,
        angle_top_q4: 200,
        layers_q4: [&LAYER, &LAYER, &LAYER],
        peaks: &PEAKS,
    };
    static STACKED_NEAR: [i16; 12] = [0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    static STACKED_MIDDLE: [i16; 12] = [0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    static STACKED_FAR: [i16; 12] = [0, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    static STACKED_PEAKS: [PeakViewPeak; 3] = [
        PeakViewPeak {
            name: "Near",
            elevation_m: 1000,
            distance_m: 1000,
            azimuth_q4: 120,
            angle_q4: 4,
            layer: 0,
            score: 1,
        },
        PeakViewPeak {
            name: "Middle",
            elevation_m: 2000,
            distance_m: 4000,
            azimuth_q4: 120,
            angle_q4: 8,
            layer: 1,
            score: 2,
        },
        PeakViewPeak {
            name: "Far",
            elevation_m: 3000,
            distance_m: 8000,
            azimuth_q4: 120,
            angle_q4: 12,
            layer: 2,
            score: 3,
        },
    ];
    static STACKED_PROFILE: PeakViewProfile = PeakViewProfile {
        id: 100,
        name: "stacked",
        observer_lat: 0,
        observer_lon: 0,
        observer_elevation_m: 0,
        default_heading_q4: 120,
        sample_step_q4: 120,
        angle_bottom_q4: -4,
        angle_top_q4: 16,
        layers_q4: [&STACKED_NEAR, &STACKED_MIDDLE, &STACKED_FAR],
        peaks: &STACKED_PEAKS,
    };

    #[test]
    fn peak_selection_wraps_across_north() {
        assert_eq!(bearing_delta_q4(40, 1400), 80);
        assert_eq!(nearest_visible_peak(&PROFILE, 0), Some(0));
        assert_eq!(nearest_visible_peak(&PROFILE, 1430), Some(2));
    }

    #[test]
    fn horizon_interpolation_wraps_to_the_first_sample() {
        assert_eq!(horizon_q4(&INTERPOLATION_LAYER, 360, 180), 2);
        assert_eq!(horizon_q4(&INTERPOLATION_LAYER, 360, 1350), 1);
    }

    #[test]
    fn a_summit_without_a_nearby_crest_anchors_at_its_nearest_sample() {
        static SHOULDER: [PeakViewPeak; 1] = [PeakViewPeak {
            name: "Shoulder",
            elevation_m: 900,
            distance_m: 2000,
            azimuth_q4: 470,
            angle_q4: 0,
            layer: 0,
            score: 1,
        }];
        static SLOPE_PROFILE: PeakViewProfile = PeakViewProfile {
            id: 101,
            name: "slope",
            observer_lat: 0,
            observer_lon: 0,
            observer_elevation_m: 0,
            default_heading_q4: 470,
            sample_step_q4: 120,
            angle_bottom_q4: -4,
            angle_top_q4: 12,
            layers_q4: [&LAYER, &LAYER, &LAYER],
            peaks: &SHOULDER,
        };
        assert_eq!(peak_anchor_q4(&SLOPE_PROFILE, 0), Some(480), "no crest within 1.5 samples of 470");
        assert!(peak_is_visible(&SLOPE_PROFILE, 0, 470), "the summit sits on its band's skyline");
    }

    #[test]
    fn a_named_summit_below_its_rendered_ridge_is_not_selectable() {
        let occluded = PeakViewPeak {
            name: "Hidden",
            elevation_m: 900,
            distance_m: 4000,
            azimuth_q4: 112,
            angle_q4: -1,
            layer: 0,
            score: 10,
        };
        assert!(!peak_reaches_ridge(&PROFILE, &occluded));
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
        state.peak_view_profile = Some(&PROFILE);
        state.compass_deg = Some(0.0);
        let mut activity = Activity::new(Mode::Idle);
        let mut settings = Settings::default();
        let mut screen = PeakViewScreen::new();
        let mut cx = test_ctx(&mut state, &mut activity, &mut settings);

        assert!(matches!(screen.handle(Gesture::Step(1), &mut cx), Transition::None));
        assert_eq!(screen.browse_heading_q4, Some(0), "Browse must not recenter the terrain on the selected peak");
        assert_eq!(screen.selected, 2, "the next summit is B on the left edge; C is outside this profile");

        screen.handle(Gesture::Step(1), &mut cx);
        assert_eq!(screen.browse_heading_q4, Some(0));
        assert_eq!(screen.selected, 0, "selection wraps among the peaks visible in the frozen profile");
    }
}
