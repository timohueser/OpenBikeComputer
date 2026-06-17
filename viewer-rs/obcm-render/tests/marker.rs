//! Smoke tests for the shared user-position marker overlay
//! ([`MapRenderer::draw_marker`]). Gated on the `render` feature so the pure-parse
//! reader build doesn't pull the graphics stack. Draws against a tiny in-memory
//! `DrawTarget` and asserts the marker lands where it should.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obcm_render::{MapRenderer, Viewport};

const RED: Rgb888 = Rgb888::new(255, 0, 0);

/// A `w`×`h` Rgb888 buffer implementing `DrawTarget`, with clipped writes.
struct Buf {
    w: i32,
    h: i32,
    px: Vec<Rgb888>,
}

impl Buf {
    fn new(w: i32, h: i32) -> Self {
        Buf { w, h, px: vec![Rgb888::BLACK; (w * h) as usize] }
    }
    fn get(&self, x: i32, y: i32) -> Rgb888 {
        self.px[(y * self.w + x) as usize]
    }
    fn count(&self, c: Rgb888) -> usize {
        self.px.iter().filter(|&&p| p == c).count()
    }
    fn put(&mut self, x: i32, y: i32, c: Rgb888) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = c;
        }
    }
}

impl OriginDimensions for Buf {
    fn size(&self) -> Size {
        Size::new(self.w as u32, self.h as u32)
    }
}

impl DrawTarget for Buf {
    type Color = Rgb888;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        for Pixel(p, c) in pixels {
            self.put(p.x, p.y, c);
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            for y in clip.top_left.y..=br.y {
                for x in clip.top_left.x..=br.x {
                    self.put(x, y, color);
                }
            }
        }
        Ok(())
    }
}

/// North-up viewport centered on (0,0); the anchor at (0,0) projects to the
/// screen center.
fn centered_vp() -> Viewport {
    Viewport::new(200.0, 200.0, 0, 0, 1.0)
}

#[test]
fn marker_fills_pixels_at_the_anchor() {
    let mut buf = Buf::new(200, 200);
    MapRenderer::new().draw_marker(&mut buf, &centered_vp(), 0, 0, Some(30.0), RED);
    assert!(buf.count(RED) > 5, "the chevron should fill a cluster of pixels");
    // The anchor projects to screen center (100,100); the glyph straddles it.
    let near_center = (95..=105).any(|y| (95..=105).any(|x| buf.get(x, y) == RED));
    assert!(near_center, "marker should sit around the projected anchor");
}

#[test]
fn stationary_fix_draws_the_dot_glyph() {
    // course = None → an orientation-free diamond, still painted at the anchor.
    let mut buf = Buf::new(200, 200);
    MapRenderer::new().draw_marker(&mut buf, &centered_vp(), 0, 0, None, RED);
    assert!(buf.count(RED) > 0);
}

#[test]
fn offscreen_anchor_is_culled() {
    // A fix far east projects way past the right edge → nothing is drawn.
    let mut buf = Buf::new(200, 200);
    MapRenderer::new().draw_marker(&mut buf, &centered_vp(), 1_000_000, 0, Some(0.0), RED);
    assert_eq!(buf.count(RED), 0);
}

#[test]
fn chevron_tip_follows_course_north_up() {
    // The farthest-right red pixel is the tip. Facing east (90°) the tip points
    // right; facing west (270°) it points left — so east reaches further right.
    let max_red_x = |course: f32| {
        let mut buf = Buf::new(200, 200);
        MapRenderer::new().draw_marker(&mut buf, &centered_vp(), 0, 0, Some(course), RED);
        let mut mx = 0;
        for y in 0..200 {
            for x in 0..200 {
                if buf.get(x, y) == RED {
                    mx = mx.max(x);
                }
            }
        }
        mx
    };
    assert!(max_red_x(90.0) > max_red_x(270.0));
}
