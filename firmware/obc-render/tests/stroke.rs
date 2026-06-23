//! Round joints/caps on thick lines. embedded-graphics joins thick `Polyline` segments with a
//! flat bevel (and butt-caps the ends), so a sharply bending curve scallops — the thick-line
//! "beading". `stroke_overlay`/`flush_run` fills a disc (⌀ = stroke width) at each **run end**
//! and at every interior vertex that bends sharply, so those joints and ends read as smooth
//! arcs. The disc at an isolated 90° corner hides inside eg's miter, so we probe the same disc
//! where it's unambiguous: a **round cap** past a line end, a pixel eg's butt-capped segment
//! can't reach. Driven through the public `stroke_path`.

use embedded_graphics::{pixelcolor::Rgb888, prelude::*, primitives::Rectangle};
use obc_render::{MapRenderer, Viewport};

const LINE: Rgb888 = Rgb888::new(255, 0, 255);

struct Buf {
    w: i32,
    h: i32,
    px: Vec<bool>,
}
impl Buf {
    fn new(w: i32, h: i32) -> Self {
        Buf { w, h, px: vec![false; (w * h) as usize] }
    }
    fn put(&mut self, x: i32, y: i32) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = true;
        }
    }
    fn on(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h && self.px[(y * self.w + x) as usize]
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
        for Pixel(p, _) in pixels {
            self.put(p.x, p.y);
        }
        Ok(())
    }
    fn fill_solid(&mut self, area: &Rectangle, _c: Self::Color) -> Result<(), Self::Error> {
        let clip = area.intersection(&self.bounding_box());
        if let Some(br) = clip.bottom_right() {
            for y in clip.top_left.y..=br.y {
                for x in clip.top_left.x..=br.x {
                    self.put(x, y);
                }
            }
        }
        Ok(())
    }
}

#[test]
fn thick_line_end_gets_a_round_cap() {
    // Camera at the equator with zoom 1 maps (lon, lat) µ° → screen (60 + lon, 30 − lat) on a
    // 120×60 buffer. A horizontal line y=30 from x=30 to x=90 (the right end at C=(90,30)).
    let vp = Viewport::new(120.0, 60.0, 0, 0, 1.0);
    let pts = [(-30, 0), (30, 0)]; // x 30→90 at y=30, in µ°
    let weight = 11u32;
    let mut buf = Buf::new(120, 60);
    MapRenderer::new().stroke_path(&mut buf, &vp, pts, LINE, weight);

    let (cx, cy) = (90, 30);
    assert!(buf.on(cx - 30, cy), "the line body is missing");
    // 3 px past the end: inside the ⌀=11 cap disc (radius ~5) but beyond eg's butt cap (which
    // stops at x=90). Painted only when the per-vertex disc — the same one that rounds interior
    // joints — is drawn.
    assert!(buf.on(cx + 3, cy), "no round cap past the line end — the per-vertex joint/cap disc isn't being drawn");
}
