//! Smoke tests for the shared user-position marker overlay
//! ([`RenderScratch::draw_marker`]). Gated on the `render` feature so the pure-parse
//! reader build doesn't pull the graphics stack. Draws against a tiny in-memory
//! `DrawTarget` and asserts the marker lands where it should.

use embedded_graphics::pixelcolor::Rgb888;
use obc_render::{RenderScratch, Viewport};

mod common;
use common::Buf;

const RED: Rgb888 = Rgb888::new(255, 0, 0);

/// North-up viewport centered on (0,0); the anchor at (0,0) projects to the
/// screen center.
fn centered_vp() -> Viewport {
    Viewport::new(200.0, 200.0, 0, 0, 1.0)
}

#[test]
fn marker_fills_pixels_at_the_anchor() {
    let mut buf = Buf::new(200, 200);
    RenderScratch::new().draw_marker(&mut buf, &centered_vp(), 0, 0, Some(30.0), RED);
    assert!(buf.count(RED) > 5, "the chevron should fill a cluster of pixels");
    // The anchor projects to screen center (100,100); the glyph straddles it.
    let near_center = (95..=105).any(|y| (95..=105).any(|x| buf.get(x, y) == RED));
    assert!(near_center, "marker should sit around the projected anchor");
}

#[test]
fn stationary_fix_draws_the_dot_glyph() {
    // course = None → an orientation-free diamond, still painted at the anchor.
    let mut buf = Buf::new(200, 200);
    RenderScratch::new().draw_marker(&mut buf, &centered_vp(), 0, 0, None, RED);
    assert!(buf.count(RED) > 0);
}

#[test]
fn offscreen_anchor_is_culled() {
    // A fix far east projects way past the right edge → nothing is drawn.
    let mut buf = Buf::new(200, 200);
    RenderScratch::new().draw_marker(&mut buf, &centered_vp(), 1_000_000, 0, Some(0.0), RED);
    assert_eq!(buf.count(RED), 0);
}

#[test]
fn chevron_tip_follows_course_north_up() {
    // The farthest-right red pixel is the tip. Facing east (90°) the tip points
    // right; facing west (270°) it points left — so east reaches further right.
    let max_red_x = |course: f32| {
        let mut buf = Buf::new(200, 200);
        RenderScratch::new().draw_marker(&mut buf, &centered_vp(), 0, 0, Some(course), RED);
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
