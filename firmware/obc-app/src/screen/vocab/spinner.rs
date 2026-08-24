//! The wait **spinner** — the free-spinning compass needle every "working…" screen fronts its wait
//! with (the nav planner's #499, the DFU check and arm waits). One type owns the sweep rate, the
//! repaint throttle, the radius, and the dirty disc it promises the host; the screens own their
//! title, their caption, and what their Back means.

use embedded_graphics::{
    prelude::{Point, Size},
    primitives::Rectangle,
};
use obc_render::Surface;

use crate::screen::ScreenTick;

/// Degrees per second the needle sweeps — a calm, steady rotation (one revolution per 1.5 s),
/// advanced by real elapsed millis so the speed reads the same at any host frame rate.
const SPIN_DPS: f32 = 240.0;

/// Frame cadence the spinner repaints at *and* asks the host to wake for — smooth enough for a
/// needle, cheap enough that a multi-second wait isn't dominated by repaints. This is a hard
/// throttle, not just a wake request: during a plan the ride loop passes by every planner step
/// (far faster than this), and each claimed repaint costs a full chrome render + push (~40 ms on
/// glass) — unthrottled, the spinner starves the work it's decorating (#500).
const SPIN_FRAME_MS: u32 = 66;

/// One tick's largest credited `dt`, in seconds. A host that was away longer (a long planner step,
/// a resumed screen) advances the needle by this much and no more, so the sweep never jumps a
/// wild multiple of a turn on the frame after a stall.
const MAX_TICK_S: f32 = 0.25;

/// The needle's sweep radius (px) — [`Spinner::draw_needle`] draws at it and [`needle_region`]
/// sizes the reported dirty disc from it, so the two can't drift.
const NEEDLE_R: f32 = 42.0;

/// The needle's base half-width (px), the shared compass needle's proportion at this radius.
const NEEDLE_HALF_W: f32 = 10.0;

/// Half-extent (px) of [`needle_region`]'s square around the needle's centre: the [`NEEDLE_R`]
/// sweep plus a rounding margin for the rasterizer (the needle's triangles round their vertices
/// away from zero, and the hub discs sit inside the sweep).
const NEEDLE_CLIP_HALF: i32 = NEEDLE_R as i32 + 2;

/// The square the spinning needle repaints inside, centred on the `(w/2, h/2)` a [`Spinner`] spins
/// at — everything else on a wait screen (title bar, name, caption) is static while the host
/// works. This is the dirty region [`Spinner::tick`] reports so the host can clip the repaint;
/// `nav.rs`'s `needle_region_covers_the_spin` pins that the sweep never escapes it on glass.
pub fn needle_region(w: i32, h: i32) -> Rectangle {
    let (cx, cy) = (w / 2, h / 2);
    Rectangle::new(
        Point::new(cx - NEEDLE_CLIP_HALF, cy - NEEDLE_CLIP_HALF),
        Size::new(2 * NEEDLE_CLIP_HALF as u32 + 1, 2 * NEEDLE_CLIP_HALF as u32 + 1),
    )
}

/// A free-spinning compass needle over static chrome — the shared "working…" indicator. Advanced
/// by real elapsed millis and throttled to [`SPIN_FRAME_MS`]; the screen holding one calls
/// [`tick`](Spinner::tick) from its `tick_timers` arm and [`draw_needle`](Spinner::draw_needle)
/// from its `draw`.
#[derive(Debug, Default)]
pub(crate) struct Spinner {
    /// Current angle (0° = N, clockwise), advanced in [`tick`](Spinner::tick).
    needle_deg: f32,
    /// Clock of the previous tick, for the per-frame `dt`; `None` before the first.
    last_ms: Option<u32>,
    /// Clock of the last tick that **claimed a repaint** — the [`SPIN_FRAME_MS`] throttle's
    /// anchor, distinct from `last_ms` (the needle advances every tick; the glass repaints at the
    /// spinner cadence).
    last_paint_ms: Option<u32>,
}

impl Spinner {
    /// Spin by real elapsed time and keep the host's frame cadence armed. `changed` is claimed at
    /// most once per [`SPIN_FRAME_MS`] no matter how often the loop passes; the needle still
    /// advances by the full elapsed time, so a throttled frame just shows a larger sweep. The
    /// claim carries [`needle_region`] as its dirty region (the chrome never changes), so the host
    /// repaints only the disc. `w`/`h` of 0 (no frame rendered yet) abstains: `None` = full
    /// repaint.
    pub(crate) fn tick(&mut self, now_ms: u32, w: i32, h: i32) -> ScreenTick {
        let dt = self.last_ms.map_or(0.0, |last| now_ms.wrapping_sub(last) as f32 / 1000.0);
        self.last_ms = Some(now_ms);
        self.needle_deg = (self.needle_deg + SPIN_DPS * dt.min(MAX_TICK_S)) % 360.0;
        let due = self.last_paint_ms.is_none_or(|last| now_ms.wrapping_sub(last) >= SPIN_FRAME_MS);
        if due {
            self.last_paint_ms = Some(now_ms);
        }
        let region = (w > 0 && h > 0).then(|| needle_region(w, h));
        ScreenTick { changed: due && dt > 0.0, next_wake_ms: Some(SPIN_FRAME_MS), region }
    }

    /// Draw the needle at the panel's centre — the shared compass needle (the Menu dial's), at the
    /// radius [`needle_region`] promises the host. Centre and radius live here, so the drawn sweep
    /// and the reported disc cannot drift apart.
    pub(crate) fn draw_needle(&self, cv: &mut impl Surface, w: i32, h: i32) {
        crate::screen::menu::draw_needle(cv, Point::new(w / 2, h / 2), self.needle_deg, NEEDLE_R, NEEDLE_HALF_W);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_render::text::{Font, TextAlign};

    /// The needle sweeps by **elapsed time**, not by tick count: two ticks 100 ms apart move it as
    /// far as one 200 ms tick, and a single long gap is credited at most [`MAX_TICK_S`] so a
    /// stalled host doesn't jump the needle a wild multiple of a turn.
    #[test]
    fn angle_follows_elapsed_time_and_clamps_one_tick() {
        let mut a = Spinner::default();
        a.tick(1_000, 240, 320); // anchors the clock; nothing elapsed yet
        assert_eq!(a.needle_deg, 0.0, "the first tick has no elapsed time to spend");
        a.tick(1_100, 240, 320);
        a.tick(1_200, 240, 320);
        let mut b = Spinner::default();
        b.tick(1_000, 240, 320);
        b.tick(1_200, 240, 320);
        assert!((a.needle_deg - b.needle_deg).abs() < 1e-3, "two short ticks sweep as far as one long one");
        assert!((a.needle_deg - SPIN_DPS * 0.2).abs() < 1e-3, "0.2 s at 240°/s is 48°");

        // A ten-second stall is credited as 0.25 s — 60°, not 2400°.
        let mut c = Spinner::default();
        c.tick(1_000, 240, 320);
        c.tick(11_000, 240, 320);
        assert!((c.needle_deg - SPIN_DPS * MAX_TICK_S).abs() < 1e-3, "one tick is clamped to {MAX_TICK_S} s");
    }

    /// The repaint claim is throttled to the frame cadence however often the loop passes — the
    /// #500 rule that keeps a spinner from starving the work it decorates.
    #[test]
    fn repaints_no_faster_than_the_frame_cadence() {
        let mut s = Spinner::default();
        assert!(!s.tick(0, 240, 320).changed, "no time elapsed, nothing to repaint");
        // One pass every 8 ms for a full second: a claim can only land on the first pass at or
        // past each 66 ms deadline, so they can never be closer together than the cadence.
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

    /// Every tick keeps the host's wake armed at the frame cadence and reports the needle disc, so
    /// the host can clip the repaint. A frame-less poll (`w`/`h` of 0) abstains instead.
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

    /// The reported dirty disc contains the whole needle raster at **every** angle — the promise
    /// the host's clipped repaint relies on, pinned here on the geometry rather than on one
    /// screen's frames.
    #[test]
    fn needle_region_contains_the_full_raster() {
        let region = needle_region(240, 320);
        let (lo, hi) = (region.top_left, region.bottom_right().unwrap());
        let mut s = Spinner::default();
        // Sweep well past a full revolution in 3° steps (240°/s × 12.5 ms).
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
