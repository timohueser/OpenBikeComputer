//! The free-spinning compass needle every "working…" screen fronts its wait with. This module owns
//! the sweep rate, the repaint throttle, the radius, and the dirty disc it promises the host; the
//! screens own their title and their caption.

use embedded_graphics::{
    prelude::{Point, Size},
    primitives::Rectangle,
};
use obc_render::Surface;

use crate::screen::ScreenTick;

/// Degrees per second the needle sweeps. It advances on real elapsed millis, so the speed is the
/// same at any host frame rate.
const SPIN_DPS: f32 = 240.0;

/// Frame cadence the spinner repaints at and asks the host to wake for. This is a hard throttle,
/// not only a wake request: callers can poll much faster than this, and each claimed repaint costs
/// a full render and push, which starves the work the spinner decorates.
const SPIN_FRAME_MS: u32 = 66;

/// One tick's largest credited `dt`, in seconds. A host that was away longer advances the needle
/// by this much and no more, so the sweep never jumps many turns after a stall.
const MAX_TICK_S: f32 = 0.25;

/// The needle's sweep radius (px). [`Spinner::draw_needle`] draws at it and [`needle_region`]
/// sizes the dirty disc from it, so the two cannot drift.
const NEEDLE_R: f32 = 42.0;

/// The needle's base half-width (px).
const NEEDLE_HALF_W: f32 = 10.0;

/// Half-extent (px) of [`needle_region`]'s square: the [`NEEDLE_R`] sweep plus a rounding margin
/// for the rasterizer.
const NEEDLE_CLIP_HALF: i32 = NEEDLE_R as i32 + 2;

/// The square the needle repaints inside, centred on `(w/2, h/2)`. Everything else on a wait
/// screen is static, so [`Spinner::tick`] reports this square and the host clips the repaint.
pub fn needle_region(w: i32, h: i32) -> Rectangle {
    let (cx, cy) = (w / 2, h / 2);
    Rectangle::new(
        Point::new(cx - NEEDLE_CLIP_HALF, cy - NEEDLE_CLIP_HALF),
        Size::new(2 * NEEDLE_CLIP_HALF as u32 + 1, 2 * NEEDLE_CLIP_HALF as u32 + 1),
    )
}

/// A free-spinning compass needle over static chrome. The screen holding one calls
/// [`tick`](Spinner::tick) from its `tick_timers` arm and [`draw_needle`](Spinner::draw_needle)
/// from its `draw`.
#[derive(Debug, Default)]
pub(crate) struct Spinner {
    /// Current angle, 0° = N, clockwise.
    needle_deg: f32,
    /// Clock of the previous tick, for the per-frame `dt`.
    last_ms: Option<u32>,
    /// Clock of the last tick that claimed a repaint. The needle advances every tick, but the
    /// glass repaints only at the spinner cadence, so this is not `last_ms`.
    last_paint_ms: Option<u32>,
}

impl Spinner {
    /// Spin by real elapsed time and keep the host's frame cadence armed. `changed` is claimed at
    /// most once per [`SPIN_FRAME_MS`], but the needle advances by the full elapsed time, so a
    /// throttled frame shows a larger sweep. `w`/`h` of 0 means no frame is rendered yet, and the
    /// reported region is `None`, which is a full repaint.
    pub(crate) fn tick(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        self.tick_at_cadence(now_ms, w, h, SPIN_FRAME_MS)
    }

    /// Keep the same sweep speed while a compute-heavy screen reserves more time for its work.
    pub(crate) fn tick_at_cadence(&mut self, now_ms: u32, w: i32, h: i32, frame_ms: u32) -> ScreenTick {
        let dt = self.last_ms.map_or(0.0, |last| now_ms.wrapping_sub(last) as f32 / 1000.0);
        self.last_ms = Some(now_ms);
        self.needle_deg = (self.needle_deg + SPIN_DPS * dt.min(MAX_TICK_S)) % 360.0;
        let due = self.last_paint_ms.is_none_or(|last| now_ms.wrapping_sub(last) >= frame_ms);
        if due {
            self.last_paint_ms = Some(now_ms);
        }
        let region = (w > 0 && h > 0).then(|| needle_region(w, h));
        ScreenTick { changed: due && dt > 0.0, next_wake_ms: Some(frame_ms), region }
    }

    /// Draw the needle at the panel's centre, at the radius [`needle_region`] promises the host.
    pub(crate) fn draw_needle(&self, cv: &mut impl Surface, w: i32, h: i32) {
        crate::screen::menu::draw_needle(cv, Point::new(w / 2, h / 2), self.needle_deg, NEEDLE_R, NEEDLE_HALF_W);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_render::text::{Font, TextAlign};

    #[test]
    fn angle_follows_elapsed_time_and_clamps_one_tick() {
        let mut a = Spinner::default();
        a.tick(1_000, 240, 320);
        assert_eq!(a.needle_deg, 0.0, "the first tick has no elapsed time to spend");
        a.tick(1_100, 240, 320);
        a.tick(1_200, 240, 320);
        let mut b = Spinner::default();
        b.tick(1_000, 240, 320);
        b.tick(1_200, 240, 320);
        assert!((a.needle_deg - b.needle_deg).abs() < 1e-3, "two short ticks sweep as far as one long one");
        assert!((a.needle_deg - SPIN_DPS * 0.2).abs() < 1e-3, "0.2 s at 240°/s is 48°");

        let mut c = Spinner::default();
        c.tick(1_000, 240, 320);
        c.tick(11_000, 240, 320);
        assert!((c.needle_deg - SPIN_DPS * MAX_TICK_S).abs() < 1e-3, "one tick is clamped to {MAX_TICK_S} s");
    }

    #[test]
    fn repaints_no_faster_than_the_frame_cadence() {
        let mut s = Spinner::default();
        assert!(!s.tick(0, 240, 320).changed, "no time elapsed, nothing to repaint");
        // One pass every 8 ms for a full second.
        let mut claimed: std::vec::Vec<u32> = std::vec::Vec::new();
        for i in 1..=125u32 {
            let now = i * 8;
            if s.tick(now, 240, 320).changed {
                claimed.push(now);
            }
        }
        assert!(claimed.len() > 10, "the spinner must actually repaint ({} claims)", claimed.len());
        for pair in claimed.windows(2) {
            assert!(
                pair[1] - pair[0] >= SPIN_FRAME_MS,
                "claims {} and {} are closer than the cadence",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn tick_reports_the_cadence_and_the_disc() {
        let mut s = Spinner::default();
        let t = s.tick(1_000, 240, 320);
        assert_eq!(t.next_wake_ms, Some(SPIN_FRAME_MS));
        assert_eq!(t.region, Some(needle_region(240, 320)));
        assert_eq!(s.tick(1_100, 0, 0).region, None, "no frame yet — no region promise");
    }

    /// A [`Surface`] that records the bounding box of everything drawn into it.
    #[derive(Default)]
    struct Extent {
        min: Option<(i32, i32, i32, i32)>,
    }

    impl Extent {
        fn point(&mut self, p: Point) {
            self.min = Some(match self.min {
                None => (p.x, p.y, p.x, p.y),
                Some((x0, y0, x1, y1)) => (x0.min(p.x), y0.min(p.y), x1.max(p.x), y1.max(p.y)),
            });
        }
        fn area(&mut self, r: Rectangle) {
            self.point(r.top_left);
            self.point(r.bottom_right().unwrap_or(r.top_left));
        }
        fn circle(&mut self, c: Point, r: u32) {
            let r = r as i32;
            self.point(Point::new(c.x - r, c.y - r));
            self.point(Point::new(c.x + r, c.y + r));
        }
    }

    impl Surface for Extent {
        fn clear(&mut self, _color: u16) {}
        fn fill(&mut self, area: Rectangle, _color: u16) {
            self.area(area);
        }
        fn round(&mut self, area: Rectangle, _radius: u32, _color: u16) {
            self.area(area);
        }
        fn round_outline(&mut self, area: Rectangle, _radius: u32, _color: u16) {
            self.area(area);
        }
        fn line(&mut self, a: Point, b: Point, _color: u16) {
            self.point(a);
            self.point(b);
        }
        fn triangle(&mut self, a: Point, b: Point, c: Point, _color: u16) {
            self.point(a);
            self.point(b);
            self.point(c);
        }
        fn disc(&mut self, center: Point, radius: u32, _color: u16) {
            self.circle(center, radius);
        }
        fn text(&mut self, _s: &str, at: Point, _f: Font, _a: TextAlign, _color: u16) -> Point {
            self.point(at);
            at
        }
    }

    #[test]
    fn needle_region_contains_the_full_raster() {
        let region = needle_region(240, 320);
        let (lo, hi) = (region.top_left, region.bottom_right().unwrap());
        let mut s = Spinner::default();
        // Sweep past a full revolution in 3° steps.
        for i in 0..=150u32 {
            s.tick(i * 13, 240, 320);
            let mut ext = Extent::default();
            s.draw_needle(&mut ext, 240, 320);
            let (x0, y0, x1, y1) = ext.min.expect("the needle draws something");
            assert!(
                x0 >= lo.x && y0 >= lo.y && x1 <= hi.x && y1 <= hi.y,
                "needle at {}° spans ({x0},{y0})..({x1},{y1}), outside {region:?}",
                s.needle_deg
            );
        }
    }
}
