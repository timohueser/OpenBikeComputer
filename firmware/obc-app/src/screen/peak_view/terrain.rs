//! Draw completed bearings from RAM and mark pending bearings without inventing terrain.
use super::{palette, COMPASS_H};
use crate::peak_view::{panorama::ROWS, Panorama, PeakViewProfile};
use obc_render::Surface;

const TONES: [u16; 4] = [palette::PARCHMENT, palette::rgb565(170, 170, 170), palette::rgb565(85, 85, 85), palette::INK];

pub(super) fn draw(
    cv: &mut impl Surface,
    terrain: Option<&Panorama>,
    profile: &PeakViewProfile,
    heading: u16,
    w: i32,
    bottom: i32,
) {
    if w < 2 || bottom <= COMPASS_H {
        return;
    }
    let fov = profile.horizontal_fov_q4();
    for x in 0..w {
        let bearing = (i32::from(heading) - fov / 2 + x * fov / (w - 1)).rem_euclid(1440) as u16;
        let Some(terrain) = terrain.filter(|terrain| terrain.ready_at_bearing_q4(bearing)) else {
            for y in (COMPASS_H + (24 - x % 24) % 24..bottom).step_by(24) {
                cv.vline(x, y, 1, 1, TONES[1]);
            }
            continue;
        };
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

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::{mock_display::MockDisplay, pixelcolor::BinaryColor, prelude::Point};

    #[test]
    fn before_the_job_exists_draws_the_same_hatch_as_unfinished_terrain() {
        let mut absent = MockDisplay::<BinaryColor>::new();
        let mut pending = MockDisplay::<BinaryColor>::new();
        let color = |_: u16| BinaryColor::On;
        let profile = PeakViewProfile::at(0, 0, 0);
        draw(&mut obc_render::Canvas::new(&mut absent, &color), None, &profile, 0, 48, 60);
        draw(&mut obc_render::Canvas::new(&mut pending, &color), Some(&Panorama::default()), &profile, 0, 48, 60);
        assert_eq!(absent, pending);
        assert_eq!(absent.get_pixel(Point::new(0, COMPASS_H)), Some(BinaryColor::On));
        assert_eq!(absent.get_pixel(Point::new(1, COMPASS_H)), None);
    }
}
