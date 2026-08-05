//! Joints, caps and body of thick lines. The span stroke (`Stroker::stroke`/`flush_run`) lays each
//! segment as a scanline-filled **rectangle** with butt ends, then fills a disc (⌀ = stroke width)
//! at each **run end** and at every interior vertex that bends sharply, so joints and ends read as
//! smooth round arcs rather than butt notches. We probe the disc where it's unambiguous — a
//! **round cap** past a line end, a pixel the bare rectangle can't reach — and check a diagonal
//! body for the per-row gaps a rotated-rectangle scanline is most prone to. Driven through the
//! public `stroke_path`.

use embedded_graphics::pixelcolor::Rgb888;
use obc_render::{RenderScratch, Viewport};

mod common;
// These tests only probe pixel coverage (painted or not), so the 1-bit `BitBuf` is the
// right recording target; aliased to `Buf` to keep the test bodies unchanged.
use common::BitBuf as Buf;

const LINE: Rgb888 = Rgb888::new(255, 0, 255);

#[test]
fn thick_line_end_gets_a_round_cap() {
    // Camera at the equator with zoom 1 maps (lon, lat) µ° → screen (60 + lon, 30 − lat) on a
    // 120×60 buffer. A horizontal line y=30 from x=30 to x=90 (the right end at C=(90,30)).
    let vp = Viewport::new(120.0, 60.0, 0, 0, 1.0);
    let pts = [(-30, 0), (30, 0)]; // x 30→90 at y=30, in µ°
    let weight = 11u32;
    let mut buf = Buf::new(120, 60);
    RenderScratch::new().stroke_path(&mut buf, &vp, pts, LINE, weight);

    let (cx, cy) = (90, 30);
    assert!(buf.on(cx - 30, cy), "the line body is missing");
    // 3 px past the end: inside the ⌀=11 cap disc (radius ~5) but beyond eg's butt cap (which
    // stops at x=90). Painted only when the per-vertex disc — the same one that rounds interior
    // joints — is drawn.
    assert!(buf.on(cx + 3, cy), "no round cap past the line end — the per-vertex joint/cap disc isn't being drawn");
}

#[test]
fn thick_line_body_matches_the_weight() {
    // Same 120×60 mapping as the cap test: a horizontal body y=30 from x=30 to x=90 at weight 11.
    // Count the painted rows in a column through the middle (clear of the end discs at x=30/x=90),
    // which is the rectangle body alone — it should come out ~11 px, not the ~13 a `weight/2`
    // half-width would round to, and no narrower than the 11 px cap disc.
    let vp = Viewport::new(120.0, 60.0, 0, 0, 1.0);
    let pts = [(-30, 0), (30, 0)];
    let mut buf = Buf::new(120, 60);
    RenderScratch::new().stroke_path(&mut buf, &vp, pts, LINE, 11);
    let body = (0..60).filter(|&y| buf.on(60, y)).count();
    assert!((9..=11).contains(&body), "weight-11 body is {body}px tall, expected ~11");
}

#[test]
fn thick_diagonal_fills_contiguously() {
    // The span stroke lays each segment as a scanline-filled rectangle; a near-45° rect is the case
    // most prone to per-row gaps, so probe a diagonal body for holes. Camera at the equator, zoom 1
    // on a 200×200 buffer maps (lon, lat) µ° → screen (100 + lon, 100 − lat). A diagonal y = x from
    // screen (40,40) to (160,160).
    let vp = Viewport::new(200.0, 200.0, 0, 0, 1.0);
    let pts = [(-60, 60), (60, -60)];
    let mut buf = Buf::new(200, 200);
    RenderScratch::new().stroke_path(&mut buf, &vp, pts, LINE, 11);

    // The whole centreline is painted — no scanline drops a row inside the body.
    for x in 46..=154 {
        assert!(buf.on(x, x), "gap in the diagonal body at ({x},{x})");
    }
    // And it's a thick band, not a hairline: ±3 px across the centre is filled (half-width ~5.5,
    // perpendicular offset (k,−k) has length k·√2), while 9 px out is clearly background.
    assert!(buf.on(103, 97) && buf.on(97, 103), "the diagonal is too thin");
    assert!(!buf.on(109, 91) && !buf.on(91, 109), "the diagonal bleeds well past its half-width");
}
