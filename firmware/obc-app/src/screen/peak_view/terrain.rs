//! Draw a window from the completed RAM panorama without accessing elevation storage.
use super::{fov_q4, palette, COMPASS_H};
use crate::peak_view::{panorama::ROWS, Panorama, PeakViewProfile};
use obc_render::Surface;

const TONES: [u16; 4] = [palette::PARCHMENT, palette::rgb565(170, 170, 170), palette::rgb565(85, 85, 85), palette::INK];

pub(super) fn draw(
    cv: &mut impl Surface,
    terrain: &Panorama,
    profile: &PeakViewProfile,
    heading: u16,
    w: i32,
    bottom: i32,
) {
    if w < 2 || bottom <= COMPASS_H {
        return;
    }
    let fov = fov_q4(profile);
    for x in 0..w {
        let bearing = (i32::from(heading) - fov / 2 + x * fov / (w - 1)).rem_euclid(1440) as u16;
        let mut start = 0;
        while start < ROWS {
            let tone = terrain.tone_at_bearing_q4(bearing, start);
            let mut end = start + 1;
            while end < ROWS && terrain.tone_at_bearing_q4(bearing, end) == tone {
                end += 1;
            }
            let y = COMPASS_H + start as i32 * (bottom - COMPASS_H) / ROWS as i32;
            let next = COMPASS_H + end as i32 * (bottom - COMPASS_H) / ROWS as i32;
            cv.vline(x, y, next - y, 1, TONES[tone as usize]);
            start = end;
        }
        if x % 8 < 4 && terrain.incomplete_at_bearing_q4(bearing) {
            cv.vline(x, COMPASS_H + 1, 2, 1, palette::AMBER);
        }
    }
}
