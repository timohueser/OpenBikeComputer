//! The **drawer sheets'** shared motion and marks (#1515).
//!
//! Both drawers are the same object seen from opposite edges: a sheet that arrives from its edge,
//! grows and shrinks with the page it shows, and marks the value already committed while the rider
//! browses alternatives. The curves and the tick live here so the two cannot drift, while each
//! drawer keeps its own durations — they are what a later animation pass tunes per sheet.

use embedded_graphics::prelude::Point;
use obc_render::Surface;

/// How much of a sheet has arrived from its edge, `0.0..=1.0`, on the open animation's ease-out
/// cubic: fast off the edge, settling onto its height.
pub(crate) fn arrived(now_ms: u32, opened_ms: u32, open_ms: u32) -> f32 {
    let t = now_ms.wrapping_sub(opened_ms).min(open_ms) as f32 / open_ms as f32;
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

/// How far a horizontal page transition has run, `0.0..=1.0`, on a smoothstep — so the outgoing and
/// incoming pages ease past each other rather than shear across.
pub(crate) fn slid(now_ms: u32, started_ms: u32, slide_ms: u32) -> f32 {
    let t = now_ms.wrapping_sub(started_ms).min(slide_ms) as f32 / slide_ms as f32;
    t * t * (3.0 - 2.0 * t)
}

/// The tick a nested value editor puts under the choice that is **already committed**, so browsing
/// alternatives never loses sight of what the device is actually set to.
pub(crate) fn committed_tick(cv: &mut impl Surface, x: i32, cy: i32, color: u16) {
    cv.line(Point::new(x - 5, cy), Point::new(x - 1, cy + 4), color);
    cv.line(Point::new(x - 1, cy + 4), Point::new(x + 6, cy - 4), color);
}

/// The x of the `i`-th of `count` evenly spaced notches across the track `x0..x1`. One notch sits at
/// each end; a single-choice track degenerates to its left edge rather than dividing by zero.
pub(crate) fn notch_x(x0: i32, x1: i32, i: u8, count: u8) -> i32 {
    match count.saturating_sub(1) {
        0 => x0,
        last => x0 + (x1 - x0) * i.min(last) as i32 / last as i32,
    }
}
