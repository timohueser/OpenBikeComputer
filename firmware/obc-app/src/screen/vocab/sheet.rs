//! The drawer sheets' shared motion and marks. The two drawers are the same object seen from
//! opposite edges, so the curves, the tick and the notch spacing live here. Each drawer keeps its
//! own durations, so tuning one sheet does not move the other.

use embedded_graphics::prelude::Point;
use obc_render::{rect, Surface};

use crate::screen::{palette, ScreenTick};

/// How much of a sheet has arrived from its edge, `0.0..=1.0`, on an ease-out cubic.
fn arrived(now_ms: u32, opened_ms: u32, open_ms: u32) -> f32 {
    let t = now_ms.wrapping_sub(opened_ms).min(open_ms) as f32 / open_ms as f32;
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

/// How far a horizontal page transition has run, `0.0..=1.0`, on a smoothstep.
fn slid(now_ms: u32, started_ms: u32, slide_ms: u32) -> f32 {
    let t = now_ms.wrapping_sub(started_ms).min(slide_ms) as f32 / slide_ms as f32;
    t * t * (3.0 - 2.0 * t)
}

/// The tick a nested value editor puts under the choice that is already committed.
pub(crate) fn committed_tick(cv: &mut impl Surface, x: i32, cy: i32, color: u16) {
    cv.line(Point::new(x - 5, cy), Point::new(x - 1, cy + 4), color);
    cv.line(Point::new(x - 1, cy + 4), Point::new(x + 6, cy - 4), color);
}

/// The x of the `i`-th of `count` evenly spaced notches across the track `x0..x1`. One notch sits
/// at each end; a single-choice track gives the left edge.
pub(crate) fn notch_x(x0: i32, x1: i32, i: u8, count: u8) -> i32 {
    match count.saturating_sub(1) {
        0 => x0,
        last => x0 + (x1 - x0) * i.min(last) as i32 / last as i32,
    }
}

/// One sheet's durations: how long it takes to arrive from its edge, how long a page transition
/// runs, and the panel step the open is quantised to. A parameter and not a constant, because the
/// two drawers are tuned apart: the step is a step the panel can finish for that sheet's width of
/// ink, so the shared curves do not predict it.
#[derive(Clone, Copy)]
pub(crate) struct SheetTiming {
    pub(crate) open_ms: u32,
    pub(crate) slide_ms: u32,
    pub(crate) step_ms: u32,
}

/// A drawer sheet's motion: the open slide, the page transition in flight, and the draw of the
/// screen below that the sheet owes. Both drawers hold one; everything that differs between them
/// — the pages, their heights, the content — stays in the drawer.
pub(crate) struct SheetMotion {
    /// When the open slide started: the clock of the first frame that could draw the sheet, and
    /// `None` until one has.
    ///
    /// A chord is resolved above the pass, before the pass sets its `now_ms`, so a sheet stamped
    /// at construction would carry the clock of the pass before the squeeze. On a host whose
    /// frames gap that is seconds old, the first frame computes an elapsed far past `open_ms`, and
    /// the sheet is drawn already landed. Starting the clock on the first tick makes the open
    /// begin where it can first be seen.
    opened_ms: Option<u32>,
    /// When the page transition in flight started, or `None` when none is.
    slide_ms: Option<u32>,
    /// How much of the sheet the last reported tick put on the panel, in device pixels; `-1`
    /// before the first one. It is what makes the open motion rather than a cut: a step that would
    /// redraw the sheet where it already stands is not reported at all.
    shown_h: i16,
    /// The draw of the screen below that this sheet owes — see [`needs_base`](Self::needs_base).
    needs_base: bool,
}

impl SheetMotion {
    /// A sheet that has begun to open. Its slide starts on the first frame that ticks it — see
    /// [`opened_ms`](Self::opened_ms).
    pub(crate) const fn opening() -> Self {
        SheetMotion { opened_ms: None, slide_ms: None, shown_h: -1, needs_base: false }
    }

    /// A sheet that is already landed: the sheet a row of another sheet swapped in. A sheet that
    /// is on the panel does not make an entrance, and re-running the open to change tables would
    /// read as a stutter, so the open's clock is stamped as spent.
    ///
    /// It owes the screen below one draw from here, because the incoming sheet may be shorter than
    /// the one it replaced and give back a band still holding the old sheet's ink. The debt is
    /// armed here rather than at the first tick, which would be one frame late.
    pub(crate) fn landed(now_ms: u32, t: SheetTiming) -> Self {
        SheetMotion { opened_ms: Some(now_ms.wrapping_sub(t.open_ms)), needs_base: true, ..SheetMotion::opening() }
    }

    /// Whether a page slide is still in flight at `now_ms`: the input gate's question, asked
    /// without answering the tick's.
    ///
    /// Retiring a slide is [`settle`](Self::settle)'s edge, and that edge is what
    /// [`tick`](Self::tick) reads to arm the base draw the settling frame owes. Input runs first in
    /// a pass, so this gate must stay a pure read: a gesture that retired the slide would leave the
    /// tick nothing to read, and the sheet would stay half-slid.
    pub(crate) fn sliding(&self, now_ms: u32, t: SheetTiming) -> bool {
        self.slide_ms.is_some_and(|s| now_ms.wrapping_sub(s) < t.slide_ms)
    }

    /// Begin a horizontal transition. The drawer makes the destination its live page at once (so
    /// its `handle` and the render key already speak about the destination) while the slide draws
    /// both.
    pub(crate) fn slide_to(&mut self, now_ms: u32) {
        self.slide_ms = Some(now_ms);
        // From this frame on the two pages travel outside the sheet's own footprint, so the base
        // has to be under them. Armed here, because the next tick would be one frame late.
        self.needs_base = true;
    }

    /// The sheet's animation: the open slide, then any page slide, at the panel's step cadence.
    /// `sheet_h` is this frame's full sheet height, which is [`height`](Self::height).
    pub(crate) fn tick(&mut self, now_ms: u32, t: SheetTiming, sheet_h: i32) -> ScreenTick {
        // This frame is the open's origin if no frame has been one yet.
        let opened_ms = *self.opened_ms.get_or_insert(now_ms);
        let settled = self.settle(now_ms, t);
        let visible = self.visible_height(now_ms, t, sheet_h);
        // The open is over when the sheet has arrived, not when its clock runs out: the ease-out's
        // last few per cent move no pixel, and the steps they would ask for push nothing.
        let opening = if sheet_h > 0 && visible >= sheet_h {
            0
        } else {
            t.open_ms.saturating_sub(now_ms.wrapping_sub(opened_ms))
        };
        let sliding = self.slide_ms.map_or(0, |s| t.slide_ms.saturating_sub(now_ms.wrapping_sub(s)));
        let moved = visible != self.shown_h as i32;
        // The base draw is a debt, so this adds to it and never clears it: a pass may tick and
        // then draw no frame at all, and only a frame that drew the base ends the obligation.
        self.needs_base |= sliding > 0 || settled;
        self.shown_h = visible as i16;
        // The wake is the time to the next step boundary, not a whole step from wherever this poll
        // landed: asking for a full step off a boundary carries the offset to the end and finishes
        // the open a step late.
        let to_step = t.step_ms - now_ms.wrapping_sub(opened_ms) % t.step_ms;
        match [opening, sliding].into_iter().filter(|r| *r > 0).min() {
            // A page slide moves its two pages across a sheet that may not change height at all, so
            // it is a change whether or not the sheet grew.
            Some(remaining) => {
                ScreenTick { changed: sliding > 0 || moved, next_wake_ms: Some(to_step.min(remaining)), region: None }
            }
            // The frame a slide ends on still differs from the one before it; the frame the open
            // ends on differs only if it moved the sheet.
            None if settled || moved => ScreenTick { changed: true, next_wake_ms: None, region: None },
            None => ScreenTick::idle(),
        }
    }

    /// Retire a finished slide, and report whether this call retired it. Only [`tick`](Self::tick)
    /// calls it: the edge it returns arms the settling frame's base draw.
    fn settle(&mut self, now_ms: u32, t: SheetTiming) -> bool {
        let done = self.slide_ms.is_some_and(|s| now_ms.wrapping_sub(s) >= t.slide_ms);
        if done {
            self.slide_ms = None;
        }
        done
    }

    /// The sheet's full height this frame: `to`, or the interpolation from `from` while a slide
    /// runs, which is how the sheet grows and shrinks with its pages.
    pub(crate) fn height(&self, now_ms: u32, t: SheetTiming, from: i32, to: i32) -> i32 {
        let Some(started) = self.slide_ms else { return to };
        let p = slid(now_ms, started, t.slide_ms);
        let (from, to) = (from as f32, to as f32);
        (from + (to - from) * p + 0.5) as i32
    }

    /// How much of `sheet_h` has arrived from the sheet's edge, on the open animation's ease-out,
    /// advanced in whole `step_ms` steps.
    ///
    /// The quantising is the pacing. A device wakes on more than its own timers, and a sheet that
    /// answered the raw clock would give a busy host many one-pixel steps, each one a whole frame
    /// the panel cannot finish. Reading the step boundary instead means the sheet moves exactly as
    /// often as it asked to be woken.
    pub(crate) fn visible_height(&self, now_ms: u32, t: SheetTiming, sheet_h: i32) -> i32 {
        // Before the first tick the open has not started, so a host that draws a sheet it has not
        // ticked draws no sheet — which is the frame the open begins from anyway.
        let Some(opened_ms) = self.opened_ms else { return 0 };
        let elapsed = now_ms.wrapping_sub(opened_ms);
        // The frame the sheet opens on is its first step, not a frame that draws nothing: the
        // chord costs the host a repaint whatever this returns. So the boundary is counted from one
        // step in.
        let stepped = (elapsed / t.step_ms + 1) * t.step_ms;
        (sheet_h as f32 * arrived(stepped, 0, t.open_ms) + 0.5) as i32
    }

    /// Where the outgoing and the incoming page sit this frame, or `None` when no slide runs.
    /// `back` pulls the outgoing page right instead of pushing it left, which is how returning to
    /// the root reads as coming back out.
    pub(crate) fn page_offsets(&self, now_ms: u32, t: SheetTiming, w: i32, back: bool) -> Option<(i32, i32)> {
        let started = self.slide_ms?;
        let p = slid(now_ms, started, t.slide_ms);
        Some(if back {
            ((p * w as f32) as i32, -((1.0 - p) * w as f32) as i32)
        } else {
            (-((p * w as f32) as i32), ((1.0 - p) * w as f32) as i32)
        })
    }

    /// Whether this sheet still owes the screen below a draw.
    ///
    /// A sheet owes one from the moment it stops purely covering the base. Three things do that: a
    /// page slide, whose two pages travel through the inset margin either side of the sheet and
    /// which shrinks the sheet when the pages differ in height; a sheet swapped in for a taller one
    /// ([`landed`](Self::landed)); and one drawer replacing the other ([`owe_base`](Self::owe_base)).
    ///
    /// It is a debt, not a flag: nothing but [`clear_base_debt`](Self::clear_base_debt) ends it. A
    /// tick that decided it per frame could have the obligation stolen by a pass that ticked and
    /// drew nothing, or by input running first and retiring the slide before the tick saw the edge.
    pub(crate) fn needs_base(&self) -> bool {
        self.needs_base
    }

    /// Discharge the debt: the frame that drew the base has put back everything this sheet was not
    /// covering. Called at the frame boundary, which is the only place that answer exists.
    pub(crate) fn clear_base_debt(&mut self) {
        self.needs_base = false;
    }

    /// Take on the debt from outside: this sheet replaces the other drawer, whose rows are still on
    /// the panel, so its first frame has to draw the base to take them off.
    pub(crate) fn owe_base(&mut self) {
        self.needs_base = true;
    }
}

/// Which edge a sheet hangs from. The sheet draws its full height with the part beyond that edge
/// off the panel, so the opposite lip is what the rider sees arriving.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Edge {
    Top,
    Bottom,
}

/// The sheet's card and its grab lip, on a `w` px wide panel. `top` is the y of the sheet's
/// on-panel edge.
pub(crate) fn frame(cv: &mut impl Surface, w: i32, top: i32, sheet_h: i32, edge: Edge) {
    // The card overhangs the edge it hangs from by its corner radius, so only the lip end reads as
    // rounded, and the sheet reads as pulled out of that edge rather than as a card.
    let (body_y, lip_y) = match edge {
        Edge::Top => (top - 8, top + sheet_h - 11),
        Edge::Bottom => (top, top + 7),
    };
    let body = rect(4, body_y, w - 8, sheet_h + 8);
    cv.round(body, 10, palette::PARCHMENT);
    cv.round_outline(body, 10, palette::WOOD_LIGHT);
    cv.round(rect(w / 2 - 18, lip_y, 36, 4), 2, palette::WOOD_LIGHT);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen::{context_drawer, quick_drawer};

    /// Both sheets as they are configured on glass. Every law below holds for each of them, which
    /// is the point of one engine: the numbers are per sheet, the behaviour is not.
    const SHEETS: [SheetTiming; 2] = [quick_drawer::MOTION, context_drawer::MOTION];

    /// A sheet of one height, ticked at `now_ms`.
    fn tick(m: &mut SheetMotion, now_ms: u32, t: SheetTiming, h: i32) -> ScreenTick {
        let sheet_h = m.height(now_ms, t, h, h);
        m.tick(now_ms, t, sheet_h)
    }

    /// The wake asks for the next step boundary, not a whole step from wherever the poll landed:
    /// otherwise a device that wakes off-boundary finishes the open a step late.
    #[test]
    fn the_wake_lands_on_the_next_step_boundary() {
        for t in SHEETS {
            let step = t.step_ms;
            let mut m = SheetMotion::opening();
            tick(&mut m, 0, t, 104); // the frame the open starts on
            assert_eq!(tick(&mut m, step + 5, t, 104).next_wake_ms, Some(step - 5), "five in, ask for the rest");
            assert_eq!(tick(&mut m, step * 2, t, 104).next_wake_ms, Some(step), "on a boundary, ask for a whole step");
        }
    }

    #[test]
    fn the_sheet_slides_in_monotonically_and_lands_exactly() {
        for t in SHEETS {
            let target = 104;
            let mut m = SheetMotion::opening();
            tick(&mut m, 1_000, t, target); // the frame the open starts on
            let quarter = t.open_ms / 4;
            let frames: heapless::Vec<i32, 8> = [0, quarter, quarter * 2, quarter * 3, t.open_ms]
                .iter()
                .map(|dt| m.visible_height(1_000 + dt, t, target))
                .collect();
            assert!(frames[0] > 0, "the sheet's first step is on the frame it opens on, not one step later");
            assert_eq!(frames[4], target, "and the sheet lands exactly on its height");
            assert!(frames.windows(2).all(|p| p[0] < p[1]), "monotonic: {frames:?}");
        }
    }

    /// The open is paced by the timing and nothing else: the sheet asks to be woken every
    /// `step_ms`, it asks for as many steps as `open_ms` pays for, and each one moves the sheet.
    /// Then it goes silent, which is what the frozen base under it depends on.
    #[test]
    fn the_open_takes_open_ms_in_steps_of_step_ms_and_every_step_moves_the_sheet() {
        for t in SHEETS {
            let target = 104;
            let mut m = SheetMotion::opening();
            let (mut ms, mut heights) = (0u32, heapless::Vec::<i32, 32>::new());
            // Poll at 1 ms, the finest any host could: what the sheet asks for is what it gets, and
            // a poll between two steps must cost nothing.
            while ms < t.open_ms * 2 {
                if tick(&mut m, ms, t, target).changed {
                    let _ = heights.push(m.visible_height(ms, t, target));
                }
                ms += 1;
            }
            assert!(heights.windows(2).all(|p| p[0] < p[1]), "no step redraws the sheet where it stands: {heights:?}");
            assert_eq!(heights.last(), Some(&target), "the last step is the sheet landed");
            // About `open_ms / step_ms` steps, allowing for the ease-out finishing early, because
            // its last few per cent move no pixel.
            let steps = heights.len() as u32;
            let expected = t.open_ms / t.step_ms;
            assert!((expected / 2..=expected + 1).contains(&steps), "{steps} steps for {expected} paid for");
            assert!(steps >= 7, "an open that reads as motion is many steps, not the four the panel used to show");

            for ms in t.open_ms * 2..t.open_ms * 2 + 500 {
                assert_eq!(tick(&mut m, ms, t, target), ScreenTick::idle(), "a landed sheet is quiet at {ms} ms");
            }
            assert!(!m.needs_base(), "…and asks for nothing under it either");
        }
    }

    /// The open starts on the frame that can first draw it, whatever the host was doing before the
    /// squeeze, so a board whose Map slept for seconds still gets the whole slide.
    #[test]
    fn the_open_starts_on_the_first_frame_and_not_on_a_clock_from_before_the_squeeze() {
        for t in SHEETS {
            let target = 104;
            // An idle Map: the pass in front of the squeeze is seconds back, and the first frame of
            // the open is the pass the chord woke.
            let first_ms = 8_000;
            let mut m = SheetMotion::opening();
            let (mut ms, mut heights) = (first_ms, heapless::Vec::<i32, 32>::new());
            while ms < first_ms + t.open_ms * 2 {
                if tick(&mut m, ms, t, target).changed {
                    let _ = heights.push(m.visible_height(ms, t, target));
                }
                ms += 1;
            }
            let first = *heights.first().expect("the open reported at least one step");
            assert!(first > 0 && first < target, "the woken frame draws the first step, not the landed sheet");
            assert_eq!(heights.last(), Some(&target), "and lands on its height");
            assert!(heights.len() >= 7, "a sparsely woken host still gets the whole slide: {heights:?}");
        }
    }

    /// A page slide asks for the base under it both ways: its two pages travel through the margin
    /// either side of the sheet, and coming back out of a taller page gives rows back. What stops
    /// it asking is the draw, not the next tick, because a tick puts no pixel back.
    ///
    /// A gesture landing exactly as the slide lands does not take that draw with it either. Input
    /// runs before the tick in one pass, so it reads the gate ([`SheetMotion::sliding`]) and
    /// retires nothing: a gesture that retired the slide would leave the tick no edge to read, and
    /// the two pages would stay half-slid. Every frame of the slide is modelled as it really runs:
    /// it draws the base, so it discharges the debt, and the next tick has to arm it again.
    #[test]
    fn a_page_slide_asks_for_the_base_until_a_frame_draws_it() {
        for t in SHEETS {
            let (root, editor) = (104, 136);
            let mut m = SheetMotion::opening();
            for ms in 0..t.open_ms * 2 {
                tick(&mut m, ms, t, root);
            }
            assert!(!m.needs_base(), "the landed root page covers what it covers");

            // Into the taller page, and through every frame of the slide.
            let start = t.open_ms * 2;
            m.slide_to(start);
            assert!(m.needs_base(), "the slide is outside the sheet from the frame it starts on");
            for ms in start..start + t.slide_ms {
                let h = m.height(ms, t, root, editor);
                assert!(m.tick(ms, t, h).changed, "a frame of the slide is a frame the host renders");
                assert!(m.needs_base(), "…and it is drawn over the base, at {ms} ms");
                m.clear_base_debt();
            }

            // The settling frame, with a gesture landing on exactly it.
            let landed = start + t.slide_ms;
            assert!(!m.sliding(landed, t), "the gate opens on the frame the slide lands");
            let h = m.height(landed, t, root, editor);
            let settling = m.tick(landed, t, h);
            assert!(m.needs_base(), "the settling frame still owes the margin the two pages travelled through");
            assert!(settling.changed, "…and is still asked for, so the pages do not stay half-slid");
            m.clear_base_debt();
            tick(&mut m, landed + 1, t, editor);
            assert!(!m.needs_base(), "the settled editor covers what it covers again");
        }
    }

    /// A swapped-in sheet owes the band it gives back, and that debt outlives passes that drew no
    /// frame: only the draw pays it, and nothing re-arms it, so a swap costs exactly one base draw.
    #[test]
    fn the_base_draw_a_sheet_owes_outlives_a_pass_that_drew_no_frame() {
        for t in SHEETS {
            let mut m = SheetMotion::landed(1_000, t);
            assert!(m.needs_base(), "the shorter sheet owes the band the taller one held");
            assert_eq!(tick(&mut m, 1_000, t, 104).next_wake_ms, None, "a landed sheet asks for no open steps");

            tick(&mut m, 1_016, t, 104);
            assert!(m.needs_base(), "two passes that rendered nothing put no pixel back");
            m.clear_base_debt();
            assert!(!m.needs_base(), "the draw is what pays it");
            tick(&mut m, 1_032, t, 104);
            assert!(!m.needs_base(), "…and a settled sheet does not ask a second time");
        }
    }
}
