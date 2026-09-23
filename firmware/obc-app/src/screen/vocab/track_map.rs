//! The track map: a route or a ride drawn on the device map inside one band, fitted to the track,
//! with a start dot, an optional end dot, and the bike-type chip in the band's lower-left corner.
//! Without a map, when the track lies outside it, or when the track is too long for the map to
//! read at the band's scale, the track draws on the plain page in the same box, with the same dots
//! and chip.

use embedded_graphics::{draw_target::DrawTarget, prelude::Point, primitives::Rectangle};
use obc_map_scene::{cos_lat, BBox};
use obc_render::{rect, Canvas, Surface, Viewport};

use super::chrome::stroke2;
use crate::app::{MAX_ZOOM, MIN_ZOOM};
use crate::screen::map::draw_track_scene;
use crate::screen::settings::bike_icons;
use crate::screen::{palette, RenderFrame};
use crate::settings::BikeType;

/// A track to preview: its decimated `(lon, lat)` polyline in microdegrees, and how it is inked.
#[derive(Clone, Copy)]
pub(crate) struct Track<'a> {
    pub points: &'a [(i32, i32)],
    pub color: u16,
    /// A route marks its destination. A ride ends where the rider stopped, so it has no end dot.
    pub end_dot: bool,
}

const DOT_R: u32 = 5;
/// The farthest scale (m/px) the band draws a map at: the coarsest bounded LOD tier of the shipped
/// map schema (`builder/presets/schema.json`). Past it one unbounded tier serves every zoom, the
/// roads drop out and a frame streams far more of the card, so a longer track draws on the plain
/// page.
const MAP_MAX_MPP: f32 = 400.0;
/// Clear space (px) between the fitted track and the edges of its box, so a dot on an extreme
/// point stays whole.
const FIT_MARGIN: f32 = 8.0;

/// The chip around the 50×30 sprite: a 3 px pad at the sides, 1 px above and 2 px below.
const CHIP_W: i32 = 56;
const CHIP_H: i32 = 33;
const CHIP_INSET: i32 = 3;

/// Draw `track` in `band` and paint everything outside the band parchment, so the caller draws the
/// page chrome over it without a clear. Draw this first: the map render clears the whole frame.
pub(crate) fn draw_track_map<D, F>(
    cv: &mut Canvas<D, F>,
    rx: &mut RenderFrame<'_, '_>,
    band: Rectangle,
    track: Track<'_>,
    bike: BikeType,
) where
    D: DrawTarget,
    F: Fn(u16) -> D::Color,
{
    use palette::*;
    let (x0, y0) = (band.top_left.x, band.top_left.y);
    let (x1, y1) = (x0 + band.size.width as i32, y0 + band.size.height as i32);
    let chip = rect(x0 + CHIP_INSET, y1 - 2 - CHIP_H, CHIP_W, CHIP_H);
    // The renderer collects what lies inside the viewport, so it ends at the band's bottom edge
    // and spends nothing on the page below.
    let vp = bounds(track.points).map(|b| (fit_clear_of(band, chip, b, track, rx.w, y1), b));

    let on_map = vp.is_some_and(|(vp, b)| {
        vp.meters_per_pixel() <= MAP_MAX_MPP
            && rx.scene.is_some_and(|scene| scene.bbox.intersects(&b))
            && draw_track_scene(cv, rx, &vp, track.points, track.color)
    });
    if on_map {
        let (w, h) = (rx.w, rx.h);
        cv.fill(rect(0, 0, w, y0), PARCHMENT);
        cv.fill(rect(0, y1, w, h - y1), PARCHMENT);
        cv.fill(rect(0, y0, x0, y1 - y0), PARCHMENT);
        cv.fill(rect(x1, y0, w - x1, y1 - y0), PARCHMENT);
    } else {
        cv.clear(PARCHMENT);
        if let Some((vp, _)) = vp {
            for pair in track.points.windows(2) {
                stroke2(cv, at(&vp, pair[0]), at(&vp, pair[1]), track.color);
            }
        }
    }

    if let Some((vp, _)) = vp {
        // A loop ends where it starts, so the start dot draws last and stays on top.
        if let (true, Some(&end)) = (track.end_dot, track.points.last()) {
            cv.disc(at(&vp, end), DOT_R, TRACK_END);
        }
        cv.disc(at(&vp, track.points[0]), DOT_R, TRACK_START);
    }

    cv.round(chip, 4, PARCHMENT);
    cv.round_outline(chip, 4, RULE);
    bike_icons::draw(
        cv,
        bike_icons::sprite(bike),
        chip.top_left.x + CHIP_W / 2,
        chip.top_left.y + 1,
        1,
        bike_icons::color(bike),
    );
}

fn at(vp: &Viewport, (lon, lat): (i32, i32)) -> Point {
    let (x, y) = vp.to_screen(lon, lat);
    Point::new(x, y)
}

/// The track's bounds, or `None` for a track too short to draw.
fn bounds(pts: &[(i32, i32)]) -> Option<BBox> {
    if pts.len() < 2 {
        return None;
    }
    let mut b = BBox { min_lon: i32::MAX, min_lat: i32::MAX, max_lon: i32::MIN, max_lat: i32::MIN };
    for &(lon, lat) in pts {
        b.min_lon = b.min_lon.min(lon);
        b.max_lon = b.max_lon.max(lon);
        b.min_lat = b.min_lat.min(lat);
        b.max_lat = b.max_lat.max(lat);
    }
    Some(b)
}

/// Fit the track into the band. When a dot would land under the chip, fit it again into the part
/// of the band right of the chip, so the chip never hides where the track starts or ends.
fn fit_clear_of(band: Rectangle, chip: Rectangle, b: BBox, track: Track<'_>, w: i32, h: i32) -> Viewport {
    let vp = fit(band, b, w, h);
    let reach = DOT_R as i32 + 1;
    let hidden = |&p: &(i32, i32)| {
        let (Point { x, y }, c) = (at(&vp, p), chip.top_left);
        x >= c.x - reach && x < c.x + CHIP_W + reach && y >= c.y - reach && y < c.y + CHIP_H + reach
    };
    let end = track.points.last().filter(|_| track.end_dot);
    if !hidden(&track.points[0]) && !end.is_some_and(hidden) {
        return vp;
    }
    let left = chip.top_left.x + CHIP_W;
    let right = band.top_left.x + band.size.width as i32;
    fit(rect(left, band.top_left.y, right - left, band.size.height as i32), b, w, h)
}

/// A north-up camera over a `w`×`h` panel that centres `b` in `area` at the largest scale that
/// keeps [`FIT_MARGIN`] clear on every side.
fn fit(area: Rectangle, b: BBox, w: i32, h: i32) -> Viewport {
    let lat = b.min_lat + (b.max_lat - b.min_lat) / 2;
    let lon = b.min_lon + (b.max_lon - b.min_lon) / 2;
    let aspect = cos_lat(lat).abs().max(0.01);
    let span_x = ((b.max_lon - b.min_lon) as f32 * aspect).max(1.0);
    let span_y = ((b.max_lat - b.min_lat) as f32).max(1.0);
    let zx = (area.size.width as f32 - 2.0 * FIT_MARGIN).max(1.0) / span_x;
    let zy = (area.size.height as f32 - 2.0 * FIT_MARGIN).max(1.0) / span_y;
    let zoom = zx.min(zy).clamp(MIN_ZOOM, MAX_ZOOM);
    let cx = area.top_left.x as f32 + area.size.width as f32 / 2.0;
    let cy = area.top_left.y as f32 + area.size.height as f32 / 2.0;
    let cam_lon = lon - ((cx - w as f32 / 2.0) / (zoom * aspect)) as i32;
    let cam_lat = lat + ((cy - h as f32 / 2.0) / zoom) as i32;
    Viewport::new(w as f32, h as f32, cam_lon, cam_lat, zoom)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAND: (i32, i32, i32, i32) = (5, 38, 230, 75);

    fn band() -> Rectangle {
        rect(BAND.0, BAND.1, BAND.2, BAND.3)
    }

    fn chip() -> Rectangle {
        rect(BAND.0 + CHIP_INSET, BAND.1 + BAND.3 - 2 - CHIP_H, CHIP_W, CHIP_H)
    }

    fn inside(r: Rectangle, (x, y): (i32, i32)) -> bool {
        let c = r.top_left;
        x >= c.x && x < c.x + r.size.width as i32 && y >= c.y && y < c.y + r.size.height as i32
    }

    /// Both ends land inside the band with the dot's margin, and neither sits under the chip, also
    /// for a track that starts in the lower-left corner where the chip is.
    #[test]
    fn the_fit_keeps_both_dots_in_the_band_and_off_the_chip() {
        let tracks: [&[(i32, i32)]; 3] = [
            // South-west to north-east: the start is where the chip is.
            &[(8_300_000, 46_500_000), (8_350_000, 46_530_000), (8_400_000, 46_560_000)],
            // North-west to south-east.
            &[(8_300_000, 46_560_000), (8_400_000, 46_500_000)],
            // A tall, narrow track.
            &[(8_300_000, 46_500_000), (8_301_000, 46_600_000)],
        ];
        for pts in tracks {
            let track = Track { points: pts, color: 0, end_dot: true };
            let b = bounds(pts).unwrap();
            let vp = fit_clear_of(band(), chip(), b, track, 240, BAND.1 + BAND.3);
            let margin = rect(BAND.0 + 6, BAND.1 + 6, BAND.2 - 12, BAND.3 - 12);
            for p in [pts[0], pts[pts.len() - 1]] {
                let s = vp.to_screen(p.0, p.1);
                assert!(inside(margin, s), "{s:?} leaves the band for {pts:?}");
                assert!(!inside(chip(), s), "{s:?} is under the chip for {pts:?}");
            }
        }
    }

    #[test]
    fn a_track_shorter_than_two_points_has_no_bounds() {
        assert!(bounds(&[]).is_none());
        assert!(bounds(&[(1, 2)]).is_none());
    }
}
