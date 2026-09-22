//! The one long name a frame scrolls, instead of cutting it with `..`. Every device font is
//! monospace, so the name scrolls by whole characters: the draw shows
//! `name[offset..offset + cells]`.
//!
//! A caller passes the pixel width of its slot, never a character count. The division into glyph
//! cells lives here alone, so no screen re-derives it.
//!
//! The frame owns the request and the runtime owns the clock. A screen's `draw` calls
//! [`MarqueeFrame::fit`] on the one name it wants scrolled and [`fit`] on every other; after the
//! draw the runtime adopts the request ([`Marquee::adopt`]) and steps it ([`Marquee::tick`]). The
//! last request of a frame wins, and a frame that asks for nothing stops the marquee.
//!
//! Cadence: rest [`HEAD_REST_MS`] at the head, one character every [`STEP_MS`] until the tail is
//! visible, rest [`TAIL_REST_MS`], return to the head.

use core::cell::Cell;

use embedded_graphics::primitives::Rectangle;
use obc_render::text::Font;

use crate::screen::ScreenTick;

pub(crate) const HEAD_REST_MS: u32 = 1_000;
pub(crate) const STEP_MS: u32 = 250;
pub(crate) const TAIL_REST_MS: u32 = 1_500;

/// One fitted name. The widest field on the panel is 18 characters.
pub(crate) type Fitted = heapless::String<64>;

/// Glyph cells of `font` that fit `budget_px`. A slot narrower than one cell holds nothing, and a
/// budget that a caller computed into the negative is a slot with no room, not a huge one.
fn cells(budget_px: i32, font: Font) -> usize {
    (budget_px / font.char_width() as i32).max(0) as usize
}

/// Fit `name` into a `budget_px` wide slot of `font`: verbatim when it fits, else the leading
/// characters plus `..`. A space before the dots is dropped, because `Fontaine du ..` reads as a
/// word.
pub(crate) fn fit(name: &str, budget_px: i32, font: Font) -> Fitted {
    let max_chars = cells(budget_px, font);
    if name.chars().count() <= max_chars {
        return window(name, 0, max_chars);
    }
    let mut out = window(name, 0, max_chars.saturating_sub(2));
    while out.ends_with(' ') {
        out.pop();
    }
    let _ = out.push_str("..");
    out
}

/// The `max_chars` characters of `name` from character `offset`.
fn window(name: &str, offset: usize, max_chars: usize) -> Fitted {
    let mut out = Fitted::new();
    for ch in name.chars().skip(offset).take(max_chars) {
        if out.push(ch).is_err() {
            break;
        }
    }
    out
}

/// The identity a scroll phase belongs to: FNV-1a over the name and its field width.
fn key_of(name: &str, max_chars: usize) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for b in name.bytes().chain((max_chars as u16).to_le_bytes()) {
        h = (h ^ u32::from(b)).wrapping_mul(0x0100_0193);
    }
    h
}

/// What a draw asked to scroll.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Request {
    key: u32,
    /// Characters beyond the field — the offset at which the tail is visible.
    steps: u16,
    region: Rectangle,
    once: bool,
}

/// The frame's view of the marquee: the offset the runtime has scrolled to, the name it belongs
/// to, and the slot a draw fills with this frame's request.
#[derive(Debug)]
pub(crate) struct MarqueeFrame {
    key: u32,
    offset: u16,
    request: Cell<Option<Request>>,
}

impl MarqueeFrame {
    /// Fit `name` into a `budget_px` wide slot of `font` and scroll it inside `scroll` when it
    /// overflows. `None` is the plain `..` cut. `scroll` is the text row the repaint clips to, in
    /// panel pixels.
    pub(crate) fn fit(&self, name: &str, budget_px: i32, font: Font, scroll: Option<Rectangle>) -> Fitted {
        match scroll {
            Some(region) => self.scrolled(name, budget_px, font, region, false),
            None => fit(name, budget_px, font),
        }
    }

    /// Like [`fit`](Self::fit), but the name scrolls once after it changes and then rests on the
    /// head.
    pub(crate) fn fit_once(&self, name: &str, budget_px: i32, font: Font, region: Rectangle) -> Fitted {
        self.scrolled(name, budget_px, font, region, true)
    }

    fn scrolled(&self, name: &str, budget_px: i32, font: Font, region: Rectangle, once: bool) -> Fitted {
        let max_chars = cells(budget_px, font);
        let len = name.chars().count();
        if len <= max_chars {
            return window(name, 0, max_chars);
        }
        let key = key_of(name, max_chars);
        let steps = (len - max_chars).min(usize::from(u16::MAX)) as u16;
        self.request.set(Some(Request { key, steps, region, once }));
        let offset = if key == self.key { self.offset.min(steps) } else { 0 };
        window(name, usize::from(offset), max_chars)
    }

    pub(crate) fn request(&self) -> Option<Request> {
        self.request.get()
    }
}

/// The runtime's marquee. One per runtime, because a frame scrolls one name.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Marquee {
    /// The adopted request and the millis it started at the head.
    shown: Option<(Request, u32)>,
    offset: u16,
}

impl Marquee {
    pub(crate) fn frame(&self) -> MarqueeFrame {
        let key = self.shown.map_or(0, |(req, _)| req.key);
        MarqueeFrame { key, offset: self.offset, request: Cell::new(None) }
    }

    /// Adopt what the frame asked for. The same name keeps its phase. A fresh name restarts at the
    /// head and returns the millis until its first step, for the render to arm, because the pass's
    /// wake was planned before the draw. No request stops the marquee.
    pub(crate) fn adopt(&mut self, request: Option<Request>, now_ms: u32) -> Option<u32> {
        match (request, self.shown) {
            (Some(req), Some((cur, start))) if req.key == cur.key => {
                self.shown = Some((req, start));
                None
            }
            (Some(req), _) => {
                self.shown = Some((req, now_ms));
                self.offset = 0;
                Some(HEAD_REST_MS)
            }
            (None, _) => {
                self.shown = None;
                self.offset = 0;
                None
            }
        }
    }

    /// Step the offset to where the clock says it is. The wake is the next step, or none once a
    /// scroll-once name is back at the head.
    pub(crate) fn tick(&mut self, now_ms: u32) -> ScreenTick {
        let Some((req, start)) = self.shown else {
            return ScreenTick::idle();
        };
        let (offset, next_wake_ms) = phase(req.steps, req.once, now_ms.wrapping_sub(start));
        let changed = offset != self.offset;
        self.offset = offset;
        ScreenTick { changed, next_wake_ms, region: Some(req.region) }
    }
}

/// The offset shown `elapsed` millis after the head, and the millis until it changes next.
fn phase(steps: u16, once: bool, elapsed: u32) -> (u16, Option<u32>) {
    let steps = u32::from(steps.max(1));
    let tail_at = HEAD_REST_MS + (steps - 1) * STEP_MS;
    let cycle = tail_at + TAIL_REST_MS;
    if once && elapsed >= cycle {
        return (0, None);
    }
    let t = elapsed % cycle;
    if t < HEAD_REST_MS {
        return (0, Some(HEAD_REST_MS - t));
    }
    let offset = ((t - HEAD_REST_MS) / STEP_MS + 1).min(steps);
    let next = if offset < steps { HEAD_REST_MS + offset * STEP_MS } else { cycle };
    (offset as u16, Some(next - t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obc_render::canvas::rect;

    const NAME: &str = "Fontaine du Mont Ventoux"; // 24 chars: 9 steps past a 15-char field
    /// A 15-cell field at the Body face, the width every test below fits into.
    const FIELD: i32 = 15 * 14;
    const ROW: Rectangle =
        Rectangle::new(embedded_graphics::prelude::Point::new(0, 100), embedded_graphics::prelude::Size::new(240, 28));
    const CYCLE: u32 = HEAD_REST_MS + 8 * STEP_MS + TAIL_REST_MS;

    fn frame(m: &mut Marquee, now_ms: u32, name: &str, once: bool) -> Fitted {
        let f = m.frame();
        let shown =
            if once { f.fit_once(name, FIELD, Font::Body, ROW) } else { f.fit(name, FIELD, Font::Body, Some(ROW)) };
        m.adopt(f.request(), now_ms);
        shown
    }

    fn shown_at(m: &mut Marquee, now_ms: u32) -> (ScreenTick, Fitted) {
        let tick = m.tick(now_ms);
        (tick, frame(m, now_ms, NAME, false))
    }

    #[test]
    fn a_name_that_fits_is_verbatim_and_never_scrolls() {
        assert_eq!(fit("Brunnen", 10 * 14, Font::Body).as_str(), "Brunnen");
        let mut m = Marquee::default();
        assert_eq!(frame(&mut m, 0, "Brunnen", false).as_str(), "Brunnen");
        assert_eq!(m, Marquee::default(), "nothing to scroll, nothing adopted");
    }

    #[test]
    fn the_cut_keeps_leading_chars_and_never_a_dangling_space() {
        assert_eq!(fit("Pass Summit Overlook", 10 * 14, Font::Body).as_str(), "Pass Sum..");
        assert_eq!(
            fit("Fontaine du Mont", 14 * 14, Font::Body).as_str(),
            "Fontaine du..",
            "the gap before the dots is dropped"
        );
        assert_eq!(fit("Brunnen", 8, Font::Body).as_str(), "..", "a slot under one cell keeps only the dots");
        assert_eq!(
            fit("Brunnen", -40, Font::Body).as_str(),
            "..",
            "and a budget computed into the negative is that slot"
        );
    }

    #[test]
    fn rests_at_the_head_then_steps_one_char_at_a_time_to_the_tail() {
        let mut m = Marquee::default();
        assert_eq!(frame(&mut m, 0, NAME, false).as_str(), "Fontaine du Mon", "the first frame is the head, no dots");
        let (t, _) = shown_at(&mut m, 0);
        assert_eq!((t.changed, t.next_wake_ms), (false, Some(HEAD_REST_MS)));
        let (t, shown) = shown_at(&mut m, HEAD_REST_MS - 1);
        assert!(!t.changed && shown.as_str() == "Fontaine du Mon", "still resting one ms before the step");
        let (t, shown) = shown_at(&mut m, HEAD_REST_MS);
        assert!(t.changed, "the first step lands exactly at the deadline");
        assert_eq!(shown.as_str(), "ontaine du Mont");
        assert_eq!(t.next_wake_ms, Some(STEP_MS));
        assert_eq!(t.region, Some(ROW), "the repaint is the text row");
        let (t, shown) = shown_at(&mut m, HEAD_REST_MS + 8 * STEP_MS);
        assert!(t.changed);
        assert_eq!(shown.as_str(), "du Mont Ventoux", "the tail is visible at the ninth step");
        assert_eq!(t.next_wake_ms, Some(TAIL_REST_MS), "then the tail rest");
    }

    #[test]
    fn loops_back_to_the_head_after_the_tail_rest() {
        let mut m = Marquee::default();
        frame(&mut m, 0, NAME, false);
        let (t, shown) = shown_at(&mut m, HEAD_REST_MS + 8 * STEP_MS);
        assert!(t.changed && shown.as_str() == "du Mont Ventoux", "a late poll lands straight on the tail");
        let (t, shown) = shown_at(&mut m, CYCLE - 1);
        assert!(!t.changed && shown.as_str() == "du Mont Ventoux", "still resting on the tail");
        let (t, shown) = shown_at(&mut m, CYCLE);
        assert!(t.changed);
        assert_eq!(shown.as_str(), "Fontaine du Mon", "back at the head");
        assert_eq!(t.next_wake_ms, Some(HEAD_REST_MS), "and resting there again");
    }

    #[test]
    fn a_scroll_once_name_returns_to_the_head_and_stops_waking() {
        let mut m = Marquee::default();
        frame(&mut m, 0, NAME, true);
        assert_eq!(m.tick(CYCLE - 1).next_wake_ms, Some(1), "the tail rest counts down");
        let t = m.tick(CYCLE);
        assert!(t.changed, "the return to the head is drawn");
        assert_eq!(t.next_wake_ms, None, "and nothing wakes for it again");
        assert_eq!(frame(&mut m, CYCLE, NAME, true).as_str(), "Fontaine du Mon");
        let later = m.tick(10 * CYCLE);
        assert!(!later.changed && later.next_wake_ms.is_none(), "the same name stays at rest");
    }

    #[test]
    fn a_fresh_name_restarts_at_the_head_and_arms_the_first_step() {
        let mut m = Marquee::default();
        frame(&mut m, 0, NAME, false);
        let (_, shown) = shown_at(&mut m, HEAD_REST_MS + STEP_MS);
        assert_eq!(shown.as_str(), "ntaine du Mont ");
        let f = m.frame();
        let shown = f.fit("Refuge du Col des Tempetes", FIELD, Font::Body, Some(ROW));
        assert_eq!(shown.as_str(), "Refuge du Col d", "a different name draws its head at once");
        assert_eq!(m.adopt(f.request(), 5_000), Some(HEAD_REST_MS), "and the render arms its first step");
        assert!(!m.tick(5_000 + HEAD_REST_MS - 1).changed);
        assert!(m.tick(5_000 + HEAD_REST_MS).changed);
    }

    #[test]
    fn a_frame_that_asks_for_nothing_stops_the_marquee() {
        let mut m = Marquee::default();
        frame(&mut m, 0, NAME, false);
        assert_eq!(frame(&mut m, 10, "Brunnen", false).as_str(), "Brunnen");
        assert_eq!(m.tick(HEAD_REST_MS + 10), ScreenTick::idle());
    }

    #[test]
    fn the_last_request_of_a_frame_wins() {
        let f = Marquee::default().frame();
        let _ = f.fit(NAME, FIELD, Font::Body, Some(ROW));
        let _ = f.fit("Refuge du Col des Tempetes", FIELD, Font::Body, Some(rect(0, 50, 1, 1)));
        assert_eq!(f.request().map(|r| r.region), Some(rect(0, 50, 1, 1)));
    }

    #[test]
    fn the_phase_survives_the_clock_wrap() {
        let start = u32::MAX - 500;
        let mut m = Marquee::default();
        frame(&mut m, start, NAME, false);
        assert!(!m.tick(start.wrapping_add(HEAD_REST_MS - 1)).changed);
        assert!(m.tick(start.wrapping_add(HEAD_REST_MS)).changed, "the first step lands past the wrap");
    }
}
