//! Elevation labels on index contours (issue #1106).
//!
//! A contour with no number is a texture; with a number it is a map. The packer bakes the level into
//! the map (OBCM v13 §5.2) and marks the *index* contour styles with the `CONTOUR_INDEX` flag
//! (§2), so this pass needs no heuristic: it labels exactly the features the cartography says are
//! labelled, and nothing else.
//!
//! **The constraint that outranks everything is frame time.** The pass therefore:
//!
//! - carries no state between frames, allocates nothing, and reads only geometry the collect pass
//!   already decoded into the frame buffers — a label costs no extra map I/O;
//! - is skipped in one branch when the frame collected no index contour ([`FrameScratch::labels`]
//!   empty), which is every map without terrain and every frame with the terrain layer suppressed,
//!   so a scene with no index contours is byte-identical to the pre-label renderer;
//! - stops walking the moment [`MAX_CONTOUR_LABELS`] are placed, so a contour-dense frame does
//!   *less* per-candidate work than a sparse one, not more.
//!
//! **Horizontal only.** The text tiers are bitmap glyph strips ([`crate::font_data`]) with no
//! rotation at any angle, and heading-up rotates the *viewport*, so along-the-line text would need a
//! new glyph blitter. The standard alternative is what the device already uses for its status chips:
//! horizontal text over a filled **knockout pill**, which breaks the contour line through the label
//! instead of laying a sticker over it. Its two colours are the host's paper and ink
//! ([`MapRenderer::set_label_colors`]) — a label is chrome, and the map has no colour that means
//! "the ground here".
//!
//! **Placement.** Anchors are cadenced by **world-space arc length** along each polyline (the
//! route-chevron precedent, [`crate::overlay`]): the ground stride is a fixed screen cadence times
//! the frame's m/px, so anchors sit on fixed ground spots and do not crawl along the line as the
//! rider pans or the map rotates. The per-feature phase is a hash of the polyline's first vertex —
//! deterministic, no RNG and no clock — which decorrelates neighbouring isolines instead of stacking
//! their labels in a column. Collisions are resolved greedily against a fixed array of reserved
//! screen boxes; a candidate that overlaps a reservation, or that would touch the viewport edge, is
//! skipped rather than drawn clipped mid-glyph.

use embedded_graphics::{
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};

use obc_map_scene::{cos_lat, ground_dist_m_cl};

use crate::text::{draw_text, text_width, Font, TextAlign};
use crate::{MapRenderer, Viewport};

/// Hard cap on labels drawn in one frame. Twelve is a cartographic call as much as a budget one: on
/// a 240×320 panel a thirteenth number is clutter, not information. It is also the frame-time
/// backstop — each label costs one small rect fill plus ~1.2 k glyph pixels, so the whole pass is
/// bounded at a few tens of k pixel ops against a frame that already clears and redraws 76 800.
pub const MAX_CONTOUR_LABELS: usize = 12;

/// How many index-contour polylines one frame can offer the label pass. Not a label cap
/// ([`MAX_CONTOUR_LABELS`] is) — a ceiling on the *candidate* list the collect pass fills, so the
/// scratch stays fixed-size. Comfortably above the number of index contours a 240×320 viewport holds
/// at any LOD (the 500 m ladder puts a handful of isolines in view even at planning zoom, each split
/// per cell); beyond it, later contours simply offer no anchors, which costs at most a differently
/// placed label and never a wrong one.
pub(crate) const MAX_LABEL_CANDIDATES: usize = 96;

/// Target on-screen gap between consecutive labels on one contour (px). The ground stride is
/// `LABEL_SPACING_PX × m/px`, recomputed per frame, so the cadence stays even at every zoom while
/// each anchor stays pinned to its ground spot.
const LABEL_SPACING_PX: f32 = 180.0;

/// The label's text tier. 12×24, so "2500" is 48 px wide — a quarter of the panel, which is why only
/// index contours are labelled.
const FONT: Font = Font::Label;

/// Knockout-pill padding around the glyph run (px, each side).
const PILL_PAD_X: i32 = 3;
const PILL_PAD_Y: i32 = 2;

/// Extra breathing room a placed label reserves beyond its pill (px, each side). Two labels that
/// merely fail to overlap still read as one blob; this is the gap that keeps them separate.
const RESERVE_MARGIN: i32 = 6;

/// A label must clear the viewport edge by this much (px). The renderer *can* clip text mid-glyph;
/// a half-drawn elevation is worse than no elevation, so a candidate that comes this close is
/// skipped and the cadence moves on.
const EDGE_MARGIN: i32 = 2;

/// Anchors one polyline may offer before the pass moves on. A hard bound on the work, for two
/// reasons that are really one — the stride is derived from the frame's m/px, and nothing outside
/// the renderer promises that stays sane:
///
/// * **Time.** At a degenerate zoom the stride floors at a millimetre while a segment stays
///   kilometres long, so a single line would test millions of anchors and reject every one for
///   leaving the viewport. Frame time is the constraint this whole pass is built around; it cannot
///   be left to depend on the camera.
/// * **Termination.** `next += stride_m` on an `f32` is a **no-op** once `next` outgrows the stride
///   by more than the mantissa can hold (a 10 km segment against a 1 mm stride), and the walk would
///   never end.
///
/// Thirty-two strides is ~5 700 px of on-screen line — far past the twelve labels the frame can hold
/// — so no reachable frame is truncated by this; it exists so no unreachable one hangs.
const MAX_ANCHORS_PER_LINE: usize = 32;

/// One index-contour polyline the frame may label: where its geometry sits in the frame points and
/// the level to print.
///
/// Recorded by the collect pass at the moment a winner is published (it is the only place the
/// feature's `level` and its style's `contour_index` flag are both in hand), so the label pass needs
/// neither a span scan nor a style lookup. Six bytes; [`MAX_LABEL_CANDIDATES`] of them are budgeted
/// in `MCU_RENDERER_BYTES`.
#[derive(Clone, Copy)]
pub(crate) struct LabelCandidate {
    /// Offset of the polyline's exterior ring in the frame points.
    pub(crate) pt_start: u16,
    /// Vertex count of that ring.
    pub(crate) pt_len: u16,
    /// The contour's elevation in metres — the label's text, printed with no unit suffix.
    pub(crate) level: i16,
}

const _: () = assert!(
    core::mem::size_of::<LabelCandidate>() == 6,
    "LabelCandidate is budgeted at 6 bytes × MAX_LABEL_CANDIDATES"
);

/// An inclusive screen box `(x0, y0, x1, y1)`.
type Box2 = (i32, i32, i32, i32);

/// Whether two screen boxes share any pixel.
#[inline]
fn overlaps(a: Box2, b: Box2) -> bool {
    !(a.2 < b.0 || a.0 > b.2 || a.3 < b.1 || a.1 > b.3)
}

/// The per-feature cadence phase, in metres along the line: a fixed hash of the polyline's first
/// vertex scaled into `0..stride_m`.
///
/// Deterministic by construction — no RNG, no clock, no frame counter — so the same map through the
/// same viewport always produces the same labels. A phase at all (rather than starting every line at
/// its first vertex) matters because isolines run parallel: without it, neighbouring contours anchor
/// at the same arc length and their labels stack into a column, and the collision pass then drops
/// all but one.
#[inline]
fn phase_m(first: (i32, i32), stride_m: f32) -> f32 {
    let h = (first.0 as u32).wrapping_mul(0x9E37_79B1) ^ (first.1 as u32).rotate_left(16).wrapping_mul(0x85EB_CA6B);
    // Top 24 bits as a 0..1 fraction: plenty of phase resolution, and exactly representable in f32.
    (h >> 8) as f32 / (1u32 << 24) as f32 * stride_m
}

/// Render `level` as decimal metres into `buf`, returning the borrowed text. Allocation-free; the
/// six bytes cover `i16::MIN`.
#[inline]
fn level_text(level: i16, buf: &mut [u8; 6]) -> &str {
    let mut v = (level as i32).unsigned_abs();
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
        if v == 0 {
            break;
        }
    }
    if level < 0 {
        i -= 1;
        buf[i] = b'-';
    }
    // Digits and `-` only: never invalid UTF-8. The fallback keeps the pass panic-free regardless.
    core::str::from_utf8(&buf[i..]).unwrap_or("")
}

impl MapRenderer {
    /// Draw this frame's index-contour elevation labels, over the finished map and under every
    /// overlay. Returns how many were placed.
    ///
    /// Walks the candidates the collect pass recorded, steps each polyline by the frame's ground
    /// stride, and places a label at the first anchor that both clears the viewport edge and misses
    /// every box already reserved. Reservation is greedy and first-come: no scoring pass, no second
    /// look at a rejected anchor, no state kept for the next frame.
    pub(crate) fn draw_contour_labels<D, F>(&self, target: &mut D, vp: &Viewport, color_fn: &F) -> usize
    where
        D: DrawTarget,
        F: Fn(u16) -> D::Color,
    {
        let candidates = self.frame.labels();
        if candidates.is_empty() {
            return 0;
        }

        let pill = color_fn(!self.label_pill_inv);
        let ink = color_fn(self.label_ink);
        // The area the map plane owns this frame: the viewport minus the host's chrome bands
        // (`set_label_insets`) minus the edge margin. A candidate must fit inside it whole.
        let (top, bottom) = self.label_inset;
        let (min_x, min_y) = (EDGE_MARGIN, EDGE_MARGIN + top as i32);
        let (max_x, max_y) = (vp.w as i32 - EDGE_MARGIN, vp.h as i32 - EDGE_MARGIN - bottom as i32);
        // The frame's ground stride, and the one cos(lat) the arc-length walk needs (the viewport
        // spans a few km — a per-vertex cosine would be the same number and a per-vertex libm call).
        let stride_m = (LABEL_SPACING_PX * vp.meters_per_pixel()).max(1e-3);
        let cl = cos_lat(vp.cam_lat);
        let cap_h = FONT.cap_height() as i32;
        let half_h = cap_h / 2 + PILL_PAD_Y;

        let mut reserved = [(0i32, 0i32, 0i32, 0i32); MAX_CONTOUR_LABELS];
        let mut placed = 0usize;

        for cand in candidates {
            if placed == MAX_CONTOUR_LABELS {
                break;
            }
            let start = cand.pt_start as usize;
            let pts = &self.frame.frame_points[start..start + cand.pt_len as usize];
            if pts.len() < 2 {
                continue;
            }
            let mut buf = [0u8; 6];
            let text = level_text(cand.level, &mut buf);
            let half_w = (text_width(text, FONT) as i32 + 2 * PILL_PAD_X) / 2;

            // Arc-length walk: `next` is the ground distance still to travel before the next anchor,
            // carried across segment boundaries so the cadence is a property of the line, not of its
            // vertex spacing.
            let mut next = phase_m(pts[0], stride_m);
            let mut anchors = 0usize;
            for w in pts.windows(2) {
                let seg = ground_dist_m_cl(w[0], w[1], cl);
                // A repeated vertex contributes no arc length; `is_finite` also rejects the NaN a
                // degenerate coordinate could produce, which would otherwise loop forever below.
                if seg <= 0.0 || !seg.is_finite() {
                    continue;
                }
                while next <= seg && anchors < MAX_ANCHORS_PER_LINE {
                    let f = next / seg;
                    next += stride_m;
                    anchors += 1;
                    let lon = w[0].0 + ((w[1].0 - w[0].0) as f32 * f) as i32;
                    let lat = w[0].1 + ((w[1].1 - w[0].1) as f32 * f) as i32;
                    let (x, y) = vp.to_screen(lon, lat);
                    let bounds = (x - half_w, y - half_h, x + half_w, y + half_h);
                    // Edge and chrome: skip rather than clip. Collision: skip rather than overdraw.
                    if bounds.0 < min_x
                        || bounds.1 < min_y
                        || bounds.2 >= max_x
                        || bounds.3 >= max_y
                        || reserved[..placed].iter().any(|&r| overlaps(r, bounds))
                    {
                        continue;
                    }
                    draw_label(target, text, bounds, pill, ink);
                    reserved[placed] = (
                        bounds.0 - RESERVE_MARGIN,
                        bounds.1 - RESERVE_MARGIN,
                        bounds.2 + RESERVE_MARGIN,
                        bounds.3 + RESERVE_MARGIN,
                    );
                    placed += 1;
                    if placed == MAX_CONTOUR_LABELS {
                        return placed;
                    }
                }
                if anchors == MAX_ANCHORS_PER_LINE {
                    break;
                }
                next -= seg;
            }
        }
        placed
    }
}

/// Paint one label: the knockout pill, then the number centred in it.
fn draw_label<D: DrawTarget>(target: &mut D, text: &str, bounds: Box2, pill: D::Color, ink: D::Color) {
    let (x0, y0, x1, y1) = bounds;
    let _ = Rectangle::new(Point::new(x0, y0), Size::new((x1 - x0) as u32, (y1 - y0) as u32))
        .into_styled(PrimitiveStyle::with_fill(pill))
        .draw(target);
    // `draw_text` anchors the glyph *cell* top; the caps start `cap_offset` rows into it, so shift
    // by that to centre the digits — not the cell's descender space — in the pill.
    let top = y0 + PILL_PAD_Y - FONT.cap_offset() as i32;
    draw_text(target, text, Point::new((x0 + x1) / 2, top), FONT, TextAlign::Center, ink);
}

#[cfg(test)]
mod tests {
    use super::{level_text, overlaps, phase_m};

    #[test]
    fn levels_format_as_bare_metres() {
        let mut buf = [0u8; 6];
        assert_eq!(level_text(2500, &mut buf), "2500");
        assert_eq!(level_text(0, &mut buf), "0");
        assert_eq!(level_text(-410, &mut buf), "-410"); // the Dead Sea is a map too
        assert_eq!(level_text(i16::MAX, &mut buf), "32767");
        assert_eq!(level_text(i16::MIN, &mut buf), "-32768");
    }

    /// The reservation predicate: touching boxes collide, boxes a pixel apart do not.
    #[test]
    fn reserved_boxes_collide_exactly_when_they_touch() {
        let a = (10, 10, 20, 20);
        assert!(overlaps(a, a));
        assert!(overlaps(a, (20, 20, 30, 30)), "a shared corner pixel is an overlap");
        assert!(!overlaps(a, (21, 10, 30, 20)), "one pixel clear to the right");
        assert!(!overlaps(a, (10, 21, 20, 30)), "one pixel clear below");
        assert!(overlaps(a, (0, 0, 100, 100)), "containment is overlap");
        assert!(overlaps((0, 0, 100, 100), a));
    }

    /// The cadence phase is a pure function of the vertex — the same frame twice is the same
    /// labels — and it stays inside one stride so no anchor is skipped by the offset alone.
    #[test]
    fn the_phase_is_deterministic_and_within_one_stride() {
        for &v in &[(0, 0), (8_500_000, 47_000_000), (-1_234_567, 890_123), (i32::MIN, i32::MAX)] {
            let p = phase_m(v, 100.0);
            assert_eq!(p, phase_m(v, 100.0), "same vertex, same phase");
            assert!((0.0..100.0).contains(&p), "phase {p} is inside the stride");
        }
        // Different lines get different phases — that is the whole point of having one.
        assert_ne!(phase_m((0, 0), 100.0), phase_m((0, 1000), 100.0));
    }
}
