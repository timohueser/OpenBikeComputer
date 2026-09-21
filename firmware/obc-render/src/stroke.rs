//! Polyline stroking: view clip, subpixel simplify, and the thick-stroke rasteriser.

use heapless::Vec;

use embedded_graphics::{
    prelude::*,
    primitives::{Polyline, PrimitiveStyle, Rectangle},
};

use crate::collect::ScreenPoint;
use crate::fill::fill_convex_quad;
use crate::viewport::{round_pt, Viewport};
use crate::MAX_SCREEN_POINTS;
use obc_map_scene::LineStyle;

/// Cohen–Sutherland outcode: bit 1 = left, 2 = right, 4 = above the top, 8 = below the bottom.
#[inline]
fn outcode(x: f32, y: f32, xmin: f32, ymin: f32, xmax: f32, ymax: f32) -> u8 {
    let mut c = 0;
    if x < xmin {
        c |= 1;
    } else if x > xmax {
        c |= 2;
    }
    if y < ymin {
        c |= 4;
    } else if y > ymax {
        c |= 8;
    }
    c
}

/// Clip segment `a`→`b` to the rectangle (Cohen–Sutherland), returning the visible sub-segment
/// rounded back to integer pixels, or `None` if it misses the rectangle entirely.
fn clip_segment(a: Point, b: Point, xmin: f32, ymin: f32, xmax: f32, ymax: f32) -> Option<(Point, Point)> {
    let (mut x0, mut y0) = (a.x as f32, a.y as f32);
    let (mut x1, mut y1) = (b.x as f32, b.y as f32);
    let mut o0 = outcode(x0, y0, xmin, ymin, xmax, ymax);
    let mut o1 = outcode(x1, y1, xmin, ymin, xmax, ymax);
    loop {
        if o0 | o1 == 0 {
            return Some((round_pt(x0, y0), round_pt(x1, y1)));
        }
        if o0 & o1 != 0 {
            return None; // both ends past the same edge — wholly outside
        }
        let o = if o0 != 0 { o0 } else { o1 };
        let (x, y) = if o & 8 != 0 {
            (x0 + (x1 - x0) * (ymax - y0) / (y1 - y0), ymax)
        } else if o & 4 != 0 {
            (x0 + (x1 - x0) * (ymin - y0) / (y1 - y0), ymin)
        } else if o & 2 != 0 {
            (xmax, y0 + (y1 - y0) * (xmax - x0) / (x1 - x0))
        } else {
            (xmin, y0 + (y1 - y0) * (xmin - x0) / (x1 - x0))
        };
        if o == o0 {
            x0 = x;
            y0 = y;
            o0 = outcode(x0, y0, xmin, ymin, xmax, ymax);
        } else {
            x1 = x;
            y1 = y;
            o1 = outcode(x1, y1, xmin, ymin, xmax, ymax);
        }
    }
}

/// Screen-space length of `a → b` in px.
#[inline]
fn dist(a: Point, b: Point) -> f32 {
    let (dx, dy) = ((b.x - a.x) as f32, (b.y - a.y) as f32);
    libm::sqrtf(dx * dx + dy * dy)
}

/// The `cos²θ` threshold below which a `weight`-px stroke's butt join is within ½ px of a round
/// joint, so the vertex needs no disc. A turn `θ` leaves a notch about `(weight/2)·sin(θ/2)` deep,
/// so sub-pixel means `sin(θ/2) ≤ 1/weight` and the cut-off cosine is `1 − 2·(1/weight)²`,
/// returned squared for the magnitude-folded test.
#[inline]
fn joint_disc_cos2(weight: u32) -> f32 {
    let sin_half = (1.0 / weight as f32).min(1.0); // ½px ÷ (weight/2)
    let cos = 1.0 - 2.0 * sin_half * sin_half;
    if cos <= 0.0 {
        0.0 // every turn discs — `turn_is_sharp`'s `dot ≤ 0` guard already covers it
    } else {
        cos * cos
    }
}

/// Whether the polyline turns sharply enough at `b` (across `a → b → c`) that its butt-join notch
/// would show — `cos²θ` below `cos2` ([`joint_disc_cos2`]). Magnitudes folded in, no `sqrt`/`acos`.
#[inline]
fn turn_is_sharp(a: Point, b: Point, c: Point, cos2: f32) -> bool {
    let (ux, uy) = ((b.x - a.x) as f32, (b.y - a.y) as f32);
    let (vx, vy) = ((c.x - b.x) as f32, (c.y - b.y) as f32);
    let dot = ux * vx + uy * vy;
    if dot <= 0.0 {
        return true; // ≥ 90° turn (or a degenerate spur): always disc it
    }
    // sharp ⇔ cosθ < √cos2 ⇔ dot² < cos2 · |u|²|v|²  (dot ≥ 0, so squaring keeps the sense)
    dot * dot < cos2 * (ux * ux + uy * uy) * (vx * vx + vy * vy)
}

/// Screen-space simplification tolerance in px. Subpixel by design: enough to fold away the
/// integer-projection staircase, under 1 px so the line never shifts a visible pixel.
const SIMPLIFY_EPS_PX: f32 = 0.75;

/// True when `p` lies within `eps` px of the line through `a` and `b`, the near-collinear test
/// [`simplify`] uses. No `sqrt`; a degenerate `a == b` falls back to `|p − a|`.
#[inline]
fn within_eps(p: Point, a: Point, b: Point, eps: f32) -> bool {
    let (abx, aby) = ((b.x - a.x) as f32, (b.y - a.y) as f32);
    let (apx, apy) = ((p.x - a.x) as f32, (p.y - a.y) as f32);
    let cross = apx * aby - apy * abx;
    let len_sq = abx * abx + aby * aby;
    let e2 = eps * eps;
    if len_sq < 1e-6 {
        return apx * apx + apy * apy <= e2; // a == b: distance to the point
    }
    cross * cross <= e2 * len_sq // (cross / len)² ≤ eps²  ⇔  perp-dist ≤ eps
}

/// Streaming one-lookahead collinear simplification: emits the first vertex, the last, and every
/// vertex bending off the line through its kept neighbours by more than `eps` px. O(1) state.
fn simplify<I, F>(points: I, eps: f32, mut emit: F)
where
    I: IntoIterator<Item = Point>,
    F: FnMut(Point),
{
    let mut anchor: Option<Point> = None; // last kept (emitted) vertex
    let mut held: Option<Point> = None; // candidate, kept only if it bends away by > eps
    for cur in points {
        match (anchor, held) {
            (None, _) => {
                anchor = Some(cur);
                emit(cur);
            }
            (Some(_), None) => held = Some(cur),
            (Some(a), Some(hp)) => {
                if within_eps(hp, a, cur, eps) {
                    held = Some(cur); // `hp` redundant — extend the straight run through it
                } else {
                    emit(hp);
                    anchor = Some(hp);
                    held = Some(cur);
                }
            }
        }
    }
    if let Some(hp) = held {
        emit(hp); // tail vertex
    }
}

/// One stroke operation's invariants and scratch, borrowed for a single polyline stroke. The thick
/// segment fill scan-converts from a stack edge record, so it needs no crossing scratch.
pub(crate) struct Stroker<'a, D: DrawTarget> {
    target: &'a mut D,
    run: &'a mut Vec<Point, MAX_SCREEN_POINTS>,
    color: D::Color,
    /// Stroke width in px, already `.max(1)`-clamped by [`Stroker::new`].
    weight: u32,
    /// How each flushed run rasterises. [`LineStyle::Solid`] by default;
    /// [`Stroker::stroke_dashed`] and [`Stroker::stroke_ticked`] set it.
    line: LineStyle,
    /// View rectangle grown by the stroke width ([`Stroker::new`]), as `(xmin, ymin, xmax, ymax)`.
    clip: (f32, f32, f32, f32),
    /// Arc length in px walked from this polyline's first vertex, counting the parts the view clip
    /// threw away. Anchoring the rhythm to the feature is what holds the pattern still on a pan.
    arc: f32,
    /// [`Stroker::arc`] at the current run's first point — the phase its dashes or ticks open with.
    run_arc: f32,
    w: i32,
    h: i32,
}

impl<'a, D: DrawTarget> Stroker<'a, D> {
    /// Borrow the target and scratch and fix this stroke's invariants: `weight` clamped to at
    /// least 1 and the clip rectangle grown by it, so an edge-hugging line keeps its thickness.
    pub(crate) fn new(
        target: &'a mut D,
        run: &'a mut Vec<Point, MAX_SCREEN_POINTS>,
        color: D::Color,
        weight: u32,
        w: i32,
        h: i32,
    ) -> Self {
        let weight = weight.max(1);
        let m = weight as f32 + 2.0; // clip margin ≥ half-width, so edge strokes still paint in
        let clip = (-m, -m, w as f32 + m, h as f32 + m);
        run.clear();
        Self { target, run, color, weight, line: LineStyle::Solid, clip, arc: 0.0, run_arc: 0.0, w, h }
    }

    /// Clip a projected overlay polyline to the view and stroke the on-screen runs. Clipping first
    /// means the stroker only pays for the visible part, which matters when the route is almost
    /// all off-screen at riding zoom, and the line splits into separate runs where it crosses.
    ///
    /// Points are simplified in screen space first, a subpixel dedup that hands the stroker far
    /// fewer segments without moving the line a visible pixel. Returns the on-screen vertex count.
    pub(crate) fn stroke<I>(&mut self, points: I) -> usize
    where
        I: IntoIterator<Item = Point>,
    {
        // Runs join, because each clipped segment starts where the previous one ended.
        let mut prev: Option<Point> = None;
        let mut drawn = 0usize;
        simplify(points, SIMPLIFY_EPS_PX, |v| {
            if let Some(a) = prev {
                drawn += self.stroke_seg(a, v);
            }
            prev = Some(v);
        });
        self.flush_run();
        drawn
    }

    /// Like [`Stroker::stroke`], but rasterises the runs as dashes, reusing the whole simplify,
    /// clip and run pipeline, so off-screen dashes cost nothing. Each run opens on the phase its
    /// start point reached along the feature, so dashes hold their ground positions on a pan.
    pub(crate) fn stroke_dashed<I>(&mut self, points: I) -> usize
    where
        I: IntoIterator<Item = Point>,
    {
        self.line = LineStyle::Dashed;
        self.stroke(points)
    }

    /// Like [`Stroker::stroke`], but rasterises each run as a solid stroke carrying regular
    /// perpendicular ticks, the cableway mark, with the tick phase anchored like the dash phase.
    pub(crate) fn stroke_ticked<I>(&mut self, points: I) -> usize
    where
        I: IntoIterator<Item = Point>,
    {
        self.line = LineStyle::Ticked;
        self.stroke(points)
    }

    /// Clip one committed segment to the view and append it to the current run, flushing where the
    /// line is discontinuous. Returns how many on-screen vertices it contributed.
    fn stroke_seg(&mut self, a: Point, b: Point) -> usize {
        let (xmin, ymin, xmax, ymax) = self.clip;
        // Only a dashed or ticked stroke has a phase to anchor; a solid one skips the arc's `sqrt`.
        let patterned = self.line != LineStyle::Solid;
        let drawn = match clip_segment(a, b, xmin, ymin, xmax, ymax) {
            None => {
                self.flush_run(); // segment wholly off-screen
                0
            }
            Some((c0, c1)) => {
                let mut drawn = 1; // c1
                                   // (Re)start a run if this segment didn't continue the previous one.
                if self.run.last().copied() != Some(c0) {
                    self.flush_run(); // still on the *previous* run's phase
                    if patterned {
                        self.run_arc = self.arc + dist(a, c0);
                    }
                    let _ = self.run.push(c0);
                    drawn += 1; // c0 enters the view here
                }
                let _ = self.run.push(c1);
                // Clipped at its far end → the line left the view here; close this run.
                if c1 != b {
                    self.flush_run();
                }
                drawn
            }
        };
        // Every segment advances the feature's arc, which keeps the phase anchored to the line.
        if patterned {
            self.arc += dist(a, b);
        }
        drawn
    }

    /// Rasterise the accumulated run, then clear it.
    ///
    /// A 1 px stroke goes through a thin Bresenham `Polyline`, the one width the span path cannot
    /// do, because a zero-width rectangle has no scanline crossings. From 2 px up it is spans: a
    /// filled rectangle per segment plus a round join or cap disc at the run ends and at every
    /// vertex that bends sharply enough to show a notch. The eg thick path measured about ten
    /// times a span stroke even at 2 px, so the split sits at 1 px.
    fn flush_run(&mut self) {
        if self.run.len() >= 2 {
            if self.line == LineStyle::Dashed {
                self.flush_run_dashed();
            } else if self.line == LineStyle::Ticked {
                self.flush_run_ticked();
            } else {
                self.flush_run_solid();
            }
        }
        self.run.clear();
    }

    /// The solid body of a run. It leaves `run` intact, because the ticked path draws the body and
    /// then walks the same run again.
    fn flush_run_solid(&mut self) {
        if self.run.len() >= 2 {
            if self.weight <= 1 {
                let _ = Polyline::new(self.run)
                    .into_styled(PrimitiveStyle::with_stroke(self.color, self.weight))
                    .draw(self.target);
            } else {
                // The body half-width is the integer disc radius, not `weight/2`, so rectangle and
                // disc come out the same thickness and an odd `weight` lands on its nominal width.
                let r = (self.weight / 2) as i32;
                let hw = r as f32;
                for i in 0..self.run.len() - 1 {
                    self.fill_thick_segment(self.run[i], self.run[i + 1], hw);
                }
                let cos2 = joint_disc_cos2(self.weight);
                let n = self.run.len();
                self.fill_disc(self.run[0].x, self.run[0].y, r);
                for i in 1..n - 1 {
                    if turn_is_sharp(self.run[i - 1], self.run[i], self.run[i + 1], cos2) {
                        self.fill_disc(self.run[i].x, self.run[i].y, r);
                    }
                }
                self.fill_disc(self.run[n - 1].x, self.run[n - 1].y, r);
            }
        }
    }

    /// Rasterise the accumulated run as screen-space dashes. The on-intervals emit with butt ends
    /// and no discs: dashes are short and straight, so the notch a disc would fill is invisible,
    /// and skipping them halves the fill cost and keeps the stripe edges crisp.
    fn flush_run_dashed(&mut self) {
        let dash = dash_len(self.weight);
        let hw = (self.weight / 2) as f32;
        let weight = self.weight;
        // Reborrow disjoint fields so the walk may read `run` while the closure writes the target.
        let target = &mut *self.target;
        let color = self.color;
        let (w, h) = (self.w, self.h);
        walk_dashes(self.run, dash, self.run_arc, |a, b| {
            if a == b {
                return; // an on-interval that rounded onto a single pixel
            }
            if weight <= 1 {
                let _ = Polyline::new(&[a, b]).into_styled(PrimitiveStyle::with_stroke(color, weight)).draw(target);
            } else {
                fill_butt_quad(target, a, b, hw, color, w, h);
            }
        });
    }

    /// Rasterise the accumulated run as a solid stroke plus regular perpendicular ticks, the
    /// cableway mark. Shape, not colour, separates it from the other thin dashed lines.
    fn flush_run_ticked(&mut self) {
        self.flush_run_solid();
        let (spacing, arm) = tick_geometry(self.weight);
        let weight = self.weight;
        let target = &mut *self.target;
        let color = self.color;
        let (w, h) = (self.w, self.h);
        walk_ticks(self.run, spacing, self.run_arc, |c, (ux, uy)| {
            // The tick is the segment normal, swept ±`arm` about the centre point.
            let (nx, ny) = (-uy * arm, ux * arm);
            let a = round_pt(c.0 - nx, c.1 - ny);
            let b = round_pt(c.0 + nx, c.1 + ny);
            if a == b {
                return;
            }
            if weight <= 1 {
                let _ = Polyline::new(&[a, b]).into_styled(PrimitiveStyle::with_stroke(color, 1)).draw(target);
            } else {
                fill_butt_quad(target, a, b, (weight / 2) as f32, color, w, h);
            }
        });
    }

    /// Lay down one segment of a thick stroke as a filled rectangle, swept ±`hw` px along its
    /// perpendicular. A zero-length segment is left to the joint or cap disc.
    fn fill_thick_segment(&mut self, a: Point, b: Point, hw: f32) {
        fill_butt_quad(self.target, a, b, hw, self.color, self.w, self.h);
    }

    /// Fill a solid disc of radius `r` px as horizontal spans, one `fill_solid` per row, rather
    /// than embedded-graphics' per-pixel `Circle`. It rounds the thick stroke's joints and caps.
    fn fill_disc(&mut self, cx: i32, cy: i32, r: i32) {
        if r < 1 {
            return;
        }
        let r2 = (r * r) as f32;
        for dy in -r..=r {
            let y = cy + dy;
            if y < 0 || y >= self.h {
                continue;
            }
            let hw = libm::sqrtf((r2 - (dy * dy) as f32).max(0.0)) as i32;
            let _ = self
                .target
                .fill_solid(&Rectangle::new(Point::new(cx - hw, y), Size::new((2 * hw + 1) as u32, 1)), self.color);
        }
    }
}

/// The four-point quad a thick-stroke segment sweeps: the endpoints offset ±`hw` px along the
/// perpendicular, each corner snapped to integer pixels. `None` for a zero-length segment. Split
/// out so the differential test harness can rasterise the exact reachable geometry.
fn butt_quad(a: Point, b: Point, hw: f32) -> Option<[Point; 4]> {
    let (ax, ay, bx, by) = (a.x as f32, a.y as f32, b.x as f32, b.y as f32);
    let (dx, dy) = (bx - ax, by - ay);
    let len = libm::sqrtf(dx * dx + dy * dy);
    if len < 1e-3 {
        return None;
    }
    let (nx, ny) = (-dy / len * hw, dx / len * hw); // perpendicular × half-width
    Some([
        round_pt(ax + nx, ay + ny),
        round_pt(bx + nx, by + ny),
        round_pt(bx - nx, by - ny),
        round_pt(ax - nx, ay - ny),
    ])
}

/// Lay down one thick-stroke segment as a filled rectangle through [`fill_convex_quad`]. Butt
/// ends, no caps: the solid stroke caps with separate joint discs. Spans round outward, so
/// adjacent quads overlap by at most 1 px and leave no hairline crack.
fn fill_butt_quad<D>(target: &mut D, a: Point, b: Point, hw: f32, color: D::Color, w: i32, h: i32)
where
    D: DrawTarget,
{
    if let Some(quad) = butt_quad(a, b, hw) {
        fill_convex_quad(target, &quad, color, w, h);
    }
}

/// Dash on and off length in screen px. Screen-space with no per-style knob: it is a function of
/// the rendered px width, and that width ramps with zoom, so a railway zoomed in gets
/// proportionally longer dashes rather than fine cross-hatching. Clamped to a legible 4 to 12 px.
fn dash_len(weight: u32) -> f32 {
    (3 * weight).clamp(4, 12) as f32
}

/// Tick spacing and arm half-length in screen px, screen-space like [`dash_len`]. The arm is short
/// and the spacing four times it, which is what reads as a cableway rather than a fat dash. The
/// arm clamps so a thick stroke does not grow whiskers.
fn tick_geometry(weight: u32) -> (f32, f32) {
    let arm = (weight + 1).clamp(2, 4) as f32;
    (4.0 * arm, arm)
}

/// Walk an already-clipped polyline and emit one tick every `spacing` px of arc length, as a
/// `(centre, unit direction)` pair. Phase accumulates across segments, so ticks stay evenly spaced
/// through a bend. `arc0` is the run's start along the whole feature, so a run starting
/// mid-feature picks the rhythm up where it left off.
fn walk_ticks<F>(run: &[Point], spacing: f32, arc0: f32, mut emit: F)
where
    F: FnMut((f32, f32), (f32, f32)),
{
    // Distance from `arc0` to the next mark: marks sit at `spacing/2 (mod spacing)` along the feature.
    let mut phase = libm::fmodf(1.5 * spacing - libm::fmodf(arc0, spacing), spacing);
    for seg in run.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let (ax, ay) = (a.x as f32, a.y as f32);
        let (dx, dy) = ((b.x - a.x) as f32, (b.y - a.y) as f32);
        let len = libm::sqrtf(dx * dx + dy * dy);
        if len < 1e-3 {
            continue; // degenerate segment; phase untouched
        }
        let (ux, uy) = (dx / len, dy / len);
        let mut t = phase;
        while t < len {
            emit((ax + ux * t, ay + uy * t), (ux, uy));
            t += spacing;
        }
        phase = t - len; // carry the remainder into the next segment
    }
}

/// Walk an already-clipped polyline and emit each "on" dash interval as a `(start, end)` pair.
/// The phase accumulates across segments and opens at `arc0`, the run's start along the whole
/// feature, so a run that re-enters the view resumes the rhythm. An interval spanning a vertex is
/// emitted as two pieces.
fn walk_dashes<F>(run: &[Point], dash: f32, arc0: f32, mut emit: F)
where
    F: FnMut(Point, Point),
{
    let period = 2.0 * dash;
    let mut phase = libm::fmodf(arc0, period); // arc position within [0, period); carries across segments
    for seg in run.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let (ax, ay) = (a.x as f32, a.y as f32);
        let (dx, dy) = ((b.x - a.x) as f32, (b.y - a.y) as f32);
        let len = libm::sqrtf(dx * dx + dy * dy);
        if len < 1e-3 {
            continue; // degenerate segment; phase untouched
        }
        let (ux, uy) = (dx / len, dy / len); // unit direction
        let mut t = 0.0_f32; // distance along this segment
        while t < len {
            let on = phase < dash;
            // Distance to the next boundary, clamped to the rest of the segment. Always > 0,
            // because phase ∈ [0, period), so `t` strictly advances.
            let remain = if on { dash - phase } else { period - phase };
            let step = remain.min(len - t);
            if on {
                let p0 = round_pt(ax + ux * t, ay + uy * t);
                let p1 = round_pt(ax + ux * (t + step), ay + uy * (t + step));
                emit(p0, p1);
            }
            t += step;
            phase += step;
            if phase >= period {
                phase -= period;
            }
        }
    }
}

/// Stroke one already-projected map line: the draw phase's `Kind::Line` arm, and the one place
/// per-feature line styling branches on the resolved style. A solid style strokes once, with or
/// without a `color2`, because casing is a separate finest-LOD pass; a dashed one draws dashes in
/// `color`, over a solid `color2` base when it carries one, which is the railway stripe.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_line<D>(
    target: &mut D,
    vp: &Viewport,
    pts: &[ScreenPoint],
    color: D::Color,
    weight: u32,
    line: LineStyle,
    color2: Option<D::Color>,
    screen: &mut Vec<Point, MAX_SCREEN_POINTS>,
) where
    D: DrawTarget,
{
    let (w, h) = (vp.w as i32, vp.h as i32);
    match (line, color2) {
        // Solid, with or without color2: casing is a separate pass.
        (LineStyle::Solid, _) => {
            Stroker::new(target, screen, color, weight, w, h).stroke(pts.iter().map(|p| p.point()));
        }
        (LineStyle::Dashed, None) => {
            Stroker::new(target, screen, color, weight, w, h).stroke_dashed(pts.iter().map(|p| p.point()));
        }
        (LineStyle::Dashed, Some(c2)) => {
            Stroker::new(target, screen, c2, weight, w, h).stroke(pts.iter().map(|p| p.point()));
            Stroker::new(target, screen, color, weight, w, h).stroke_dashed(pts.iter().map(|p| p.point()));
        }
        // Ticked draws its own solid body, so `color2` has nothing left to case and is ignored.
        (LineStyle::Ticked, _) => {
            Stroker::new(target, screen, color, weight, w, h).stroke_ticked(pts.iter().map(|p| p.point()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        butt_quad, dash_len, joint_disc_cos2, simplify, tick_geometry, turn_is_sharp, walk_dashes, walk_ticks,
        within_eps, LineStyle, Stroker,
    };
    use crate::fill::{fill_convex_quad, fill_polygon};
    use crate::{MAX_CROSSINGS, MAX_SCREEN_POINTS};
    use embedded_graphics::{pixelcolor::BinaryColor, prelude::*, primitives::Rectangle};
    use heapless::Vec;

    /// Collect the vertices [`simplify`] keeps from `pts` at tolerance `eps`.
    fn kept(pts: &[Point], eps: f32) -> Vec<Point, 64> {
        let mut out = Vec::new();
        simplify(pts.iter().copied(), eps, |p| {
            let _ = out.push(p);
        });
        out
    }

    /// Collect the on-dash intervals [`walk_dashes`] emits for `run`, opening at feature-arc `arc0`.
    fn dashes_from(run: &[Point], dash: f32, arc0: f32) -> Vec<(Point, Point), 64> {
        let mut out = Vec::new();
        walk_dashes(run, dash, arc0, |a, b| {
            let _ = out.push((a, b));
        });
        out
    }

    /// [`dashes_from`] for a run that starts at the feature's own first vertex.
    fn dashes(run: &[Point], dash: f32) -> Vec<(Point, Point), 64> {
        dashes_from(run, dash, 0.0)
    }

    /// Arc length of an axis-aligned or straight `a → b`.
    fn seg_len((a, b): (Point, Point)) -> f32 {
        libm::sqrtf(((b.x - a.x) * (b.x - a.x) + (b.y - a.y) * (b.y - a.y)) as f32)
    }

    #[test]
    fn within_eps_is_perpendicular_distance() {
        let (a, b) = (Point::new(0, 0), Point::new(10, 0)); // the x-axis
        assert!(within_eps(Point::new(5, 0), a, b, 0.5), "on the line");
        assert!(!within_eps(Point::new(5, 1), a, b, 0.5), "1 px off > 0.5 tol");
        assert!(within_eps(Point::new(5, 1), a, b, 1.5), "1 px off < 1.5 tol");
        // Degenerate a == b falls back to the point distance |p − a|.
        assert!(within_eps(Point::new(0, 1), a, a, 1.5));
        assert!(!within_eps(Point::new(0, 2), a, a, 1.5));
    }

    #[test]
    fn turn_is_sharp_discs_only_notch_corners() {
        let cos2 = joint_disc_cos2(11); // route weight ⇒ ~10° cut-off
        let b = Point::new(100, 0);
        // Collinear continuation: never a disc.
        assert!(!turn_is_sharp(Point::new(0, 0), b, Point::new(200, 0), cos2));
        // A ~6° bend stays under the cut-off — the butt-join notch is sub-pixel: no disc.
        assert!(!turn_is_sharp(Point::new(0, 0), b, Point::new(200, 10), cos2));
        // A ~27° bend clears it: disc.
        assert!(turn_is_sharp(Point::new(0, 0), b, Point::new(200, 50), cos2));
        // A right-angle and a hairpin (non-positive dot) always disc.
        assert!(turn_is_sharp(Point::new(0, 0), b, Point::new(100, 50), cos2));
        assert!(turn_is_sharp(Point::new(0, 0), b, Point::new(0, 10), cos2));
        // A thinner stroke tolerates a wider bend before the notch shows (looser cut-off).
        assert!(joint_disc_cos2(3) < joint_disc_cos2(11));
    }

    #[test]
    fn simplify_collapses_the_subpixel_staircase() {
        // A straight line the integer projection turned into a staircase: every point sits within
        // ½ px of the true line, so a subpixel tolerance drops all but the ends.
        let mut pts = Vec::<Point, 64>::new();
        for x in 0..=30 {
            let _ = pts.push(Point::new(x, libm::roundf(x as f32 * 0.4) as i32));
        }
        let out = kept(&pts, 0.75);
        assert!(out.len() <= 3, "staircase should collapse to ~the endpoints, kept {}", out.len());
        assert_eq!(out.first(), pts.first(), "keeps the start");
        assert_eq!(out.last(), pts.last(), "keeps the end");
    }

    #[test]
    fn dash_len_scales_with_weight_and_clamps() {
        assert_eq!(dash_len(1), 4.0, "thin: 3 clamps up to the 4 px floor");
        assert_eq!(dash_len(2), 6.0, "rail weight: 3×2");
        assert_eq!(dash_len(4), 12.0, "hits the ceiling exactly");
        assert_eq!(dash_len(9), 12.0, "clamps down to the 12 px ceiling");
    }

    /// The tick rhythm: evenly spaced marks that keep their spacing through a bend.
    #[test]
    fn walk_ticks_spaces_evenly_and_carries_its_phase_through_a_bend() {
        let (spacing, arm) = tick_geometry(1);
        assert_eq!((spacing, arm), (8.0, 2.0), "a one-pixel lift: a 2 px arm every 8 px");
        assert_eq!(tick_geometry(9), (16.0, 4.0), "the arm clamps, and the spacing follows it");

        // A 20 px horizontal run then 20 px vertical: one continuous 40 px arc.
        let run = [Point::new(0, 0), Point::new(20, 0), Point::new(20, 20)];
        let mut at: Vec<(i32, i32), 8> = Vec::new();
        let mut dirs: Vec<(f32, f32), 8> = Vec::new();
        walk_ticks(&run, spacing, 0.0, |c, d| {
            at.push((c.0 as i32, c.1 as i32)).expect("the run holds few enough ticks");
            dirs.push(d).expect("the run holds few enough ticks");
        });
        // Marks fall every 8 px from half a spacing, and the phase carries across the vertex.
        assert_eq!(at.as_slice(), [(4, 0), (12, 0), (20, 0), (20, 8), (20, 16)]);
        assert_eq!(dirs[0], (1.0, 0.0), "along the first segment");
        assert_eq!(dirs[4], (0.0, 1.0), "and along the second");

        // A run entering mid-feature resumes that rhythm rather than restarting at half a spacing.
        let mut at: Vec<(i32, i32), 8> = Vec::new();
        walk_ticks(&[Point::new(0, 0), Point::new(20, 0)], spacing, 22.0, |c, _| {
            at.push((c.0 as i32, c.1 as i32)).expect("the run holds few enough ticks");
        });
        assert_eq!(at.as_slice(), [(6, 0), (14, 0)]);
    }

    #[test]
    fn walk_dashes_alternates_on_off_from_the_run_start() {
        // A straight 20 px run at dash 4 gives period 8: on, off, on, off, on.
        let out = dashes(&[Point::new(0, 0), Point::new(20, 0)], 4.0);
        assert_eq!(
            &out[..],
            &[
                (Point::new(0, 0), Point::new(4, 0)),
                (Point::new(8, 0), Point::new(12, 0)),
                (Point::new(16, 0), Point::new(20, 0)),
            ]
        );
    }

    #[test]
    fn walk_dashes_opens_on_the_feature_arc() {
        // A clipped run carries the phase its entry point reached, so the dashes sit where the
        // unclipped line would have put them: the view's edge is not an origin.
        let run = [Point::new(5, 5), Point::new(13, 5)];
        // Entering 2 px into an "on" dash paints its remainder, then the gap, then the next dash.
        let out = dashes_from(&run, 4.0, 10.0);
        assert_eq!(&out[..], &[(Point::new(5, 5), Point::new(7, 5)), (Point::new(11, 5), Point::new(13, 5))]);
        // Entering mid-gap opens with the gap, not with ink.
        let out = dashes_from(&run, 4.0, 5.0);
        assert_eq!(&out[..], &[(Point::new(8, 5), Point::new(12, 5))]);
    }

    #[test]
    fn walk_dashes_carries_phase_across_a_vertex() {
        // An L-bend where a single "on" dash straddles the corner must split into two pieces that
        // meet at the vertex, with the gap that follows continuing on the second arm.
        let run = [Point::new(0, 0), Point::new(3, 0), Point::new(3, 3)];
        let out = dashes(&run, 4.0);
        assert_eq!(out.len(), 2, "the vertex-straddling dash splits into two pieces");
        assert_eq!(out[0], (Point::new(0, 0), Point::new(3, 0)), "first arm up to the vertex");
        assert_eq!(out[1], (Point::new(3, 0), Point::new(3, 1)), "continues onto the second arm");
        assert_eq!(out[0].1, out[1].0, "the two pieces meet at the vertex — no gap, no overlap");
        let on_total: f32 = out.iter().map(|&p| seg_len(p)).sum();
        assert!((on_total - 4.0).abs() < 1e-3, "the split preserves the 4 px on-length, got {on_total}");
    }

    #[test]
    fn walk_dashes_ignores_degenerate_segments() {
        // A repeated vertex leaves the phase untouched; clip and simplify can hand us these.
        let with_dup = dashes(&[Point::new(0, 0), Point::new(10, 0), Point::new(10, 0), Point::new(20, 0)], 4.0);
        let without = dashes(&[Point::new(0, 0), Point::new(10, 0), Point::new(20, 0)], 4.0);
        assert_eq!(&with_dup[..], &without[..]);
    }

    #[test]
    fn simplify_keeps_a_real_corner() {
        // A right-angle L: the straight arms collapse, but the corner survives.
        let mut pts = Vec::<Point, 64>::new();
        for x in 0..=10 {
            let _ = pts.push(Point::new(x, 0));
        }
        for y in 1..=10 {
            let _ = pts.push(Point::new(10, y));
        }
        let out = kept(&pts, 0.75);
        assert_eq!(out.len(), 3, "start, corner, end");
        assert_eq!(out[1], Point::new(10, 0), "the corner is kept");
    }

    // The specialized [`fill_convex_quad`] must produce a framebuffer byte-for-byte identical to
    // the general even-odd [`fill_polygon`] for every quad the stroker can hand it, so these tests
    // rasterise the exact reachable geometry through both fillers and compare the pixels.

    const GW: i32 = 44;
    const GH: i32 = 40;
    const GN: usize = (GW * GH) as usize;

    /// A tiny 1-bpp framebuffer recording which pixels each filler paints. Both reach pixels only
    /// through `fill_solid`; `draw_iter` is implemented too, so the comparison misses no path.
    struct Grid {
        px: [u8; GN],
    }
    impl Grid {
        fn new() -> Self {
            Grid { px: [0; GN] }
        }
        fn set(&mut self, x: i32, y: i32, on: bool) {
            if (0..GW).contains(&x) && (0..GH).contains(&y) {
                self.px[(y * GW + x) as usize] = on as u8;
            }
        }
    }
    impl OriginDimensions for Grid {
        fn size(&self) -> Size {
            Size::new(GW as u32, GH as u32)
        }
    }
    impl DrawTarget for Grid {
        type Color = BinaryColor;
        type Error = core::convert::Infallible;
        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(p, c) in pixels {
                self.set(p.x, p.y, c == BinaryColor::On);
            }
            Ok(())
        }
        fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
            let on = color == BinaryColor::On;
            for dy in 0..area.size.height as i32 {
                for dx in 0..area.size.width as i32 {
                    self.set(area.top_left.x + dx, area.top_left.y + dy, on);
                }
            }
            Ok(())
        }
    }

    /// Fill `quad` through both fillers, returning the two framebuffers for comparison.
    fn draw_both(quad: &[Point; 4]) -> (Grid, Grid) {
        let mut generic = Grid::new();
        let mut xs: Vec<f32, MAX_CROSSINGS> = Vec::new();
        fill_polygon(&mut generic, quad, &[4usize], BinaryColor::On, GW, GH, &mut xs);
        let mut specialized = Grid::new();
        fill_convex_quad(&mut specialized, quad, BinaryColor::On, GW, GH);
        (generic, specialized)
    }

    #[test]
    fn convex_quad_fill_matches_even_odd_on_a_reachable_grid() {
        // A deterministic grid of segment endpoints — negative, zero, mid-screen, the last
        // column, the edge and beyond — crossed with the reachable half-width range. Every quad
        // it produces is one the stroker would sweep, so a mismatch is a real pixel drift.
        let coords = [-6, -1, 0, 1, 7, 20, 33, 43, 44, 50];
        let hws = [1.0f32, 1.5, 2.0, 2.5, 3.0, 4.0, 5.5];
        let mut checked = 0usize;
        for &ax in &coords {
            for &ay in &coords {
                for &bx in &coords {
                    for &by in &coords {
                        for &hw in &hws {
                            if let Some(quad) = butt_quad(Point::new(ax, ay), Point::new(bx, by), hw) {
                                let (g, s) = draw_both(&quad);
                                assert!(
                                    g.px == s.px,
                                    "pixel drift for a=({ax},{ay}) b=({bx},{by}) hw={hw} quad={quad:?}"
                                );
                                checked += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(checked > 10_000, "grid should exercise many quads, only {checked}");
    }

    #[test]
    fn convex_quad_fill_matches_even_odd_on_pseudo_random_quads() {
        // A linear-congruential stream of endpoints well outside every screen edge, with widths
        // up to about 10 px, to shake out one-pixel deltas and shallow quads the grid misses.
        let mut state: u32 = 0x1234_5678;
        let mut next = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            state
        };
        for _ in 0..60_000 {
            let coord = |r: u32| ((r >> 9) % 130) as i32 - 45; // [-45, 84]
            let ax = coord(next());
            let ay = coord(next());
            let bx = coord(next());
            let by = coord(next());
            let hw = 1.0 + ((next() >> 8) % 950) as f32 / 100.0; // [1.0, ~10.5]
            if let Some(quad) = butt_quad(Point::new(ax, ay), Point::new(bx, by), hw) {
                let (g, s) = draw_both(&quad);
                assert!(g.px == s.px, "pixel drift for a=({ax},{ay}) b=({bx},{by}) hw={hw} quad={quad:?}");
            }
        }
    }

    #[test]
    fn convex_quad_fill_matches_even_odd_on_degenerate_quads() {
        // Hand-built quads `round_pt` can collapse a stroke into, plus non-convex and
        // self-intersecting shapes that force the more-than-two-crossings even-odd fallback.
        let quads: &[[Point; 4]] = &[
            [Point::new(5, 5), Point::new(5, 5), Point::new(5, 5), Point::new(5, 5)], // zero-area point
            [Point::new(2, 2), Point::new(10, 2), Point::new(10, 2), Point::new(2, 2)], // collinear line
            [Point::new(5, 3), Point::new(6, 3), Point::new(6, 20), Point::new(5, 20)], // 1px vertical sliver
            [Point::new(3, 5), Point::new(20, 5), Point::new(20, 6), Point::new(3, 6)], // 1px horizontal sliver
            [Point::new(4, 4), Point::new(4, 4), Point::new(20, 10), Point::new(4, 18)], // collapsed to triangle
            [Point::new(4, 18), Point::new(20, 10), Point::new(4, 4), Point::new(4, 4)], // reversed winding
            [Point::new(0, 0), Point::new(10, 10), Point::new(10, 0), Point::new(0, 10)], // bowtie → 4 crossings
            [Point::new(-30, -30), Point::new(-20, -30), Point::new(-20, -10), Point::new(-30, -10)], // fully outside
            [Point::new(-8, 12), Point::new(60, 15), Point::new(60, 18), Point::new(-8, 15)], // spans both x edges
        ];
        for quad in quads {
            let (g, s) = draw_both(quad);
            assert!(g.px == s.px, "pixel drift for degenerate quad {quad:?}");
        }
    }

    /// Stroke `pts` through the real [`Stroker`] into a [`Grid`], styled as `line`.
    fn stroked(pts: &[Point], line: LineStyle) -> Grid {
        let mut grid = Grid::new();
        let mut run: Vec<Point, MAX_SCREEN_POINTS> = Vec::new();
        let mut s = Stroker::new(&mut grid, &mut run, BinaryColor::On, 1, GW, GH);
        match line {
            LineStyle::Dashed => s.stroke_dashed(pts.iter().copied()),
            LineStyle::Ticked => s.stroke_ticked(pts.iter().copied()),
            LineStyle::Solid => s.stroke(pts.iter().copied()),
        };
        grid
    }

    /// A camera pan translates every projected vertex by the same amount, so a dashed or ticked
    /// line must keep painting the same ground positions. These lines run well past both side
    /// edges, the case whose phase would otherwise slide along the line as the view moves.
    ///
    /// The clip entry is rounded to a pixel before its arc is measured, so the phase carries up
    /// to half a pixel of rounding error that differs between frames. These geometries have exact
    /// entry arcs, which is what lets this assert pixel equality.
    #[test]
    fn a_pan_does_not_slide_the_pattern_along_the_line() {
        let cases: [&[Point]; 3] = [
            &[Point::new(-70, 11), Point::new(110, 11)],                   // horizontal
            &[Point::new(-70, 9), Point::new(-20, 9), Point::new(110, 9)], // horizontal, two segments
            &[Point::new(-70, -48), Point::new(110, 132)],                 // 45°
        ];
        for pts in cases {
            for line in [LineStyle::Dashed, LineStyle::Ticked] {
                let still = stroked(pts, line);
                for pan in 1..=9i32 {
                    let mut moved: Vec<Point, 8> = Vec::new();
                    for p in pts {
                        let _ = moved.push(Point::new(p.x - pan, p.y));
                    }
                    let panned = stroked(&moved, line);
                    // Column x of the still frame is column x − pan of the panned one.
                    for y in 0..GH {
                        for x in pan..GW {
                            let (a, b) = (still.px[(y * GW + x) as usize], panned.px[(y * GW + x - pan) as usize]);
                            assert_eq!(a, b, "{line:?} pattern moved at ({x},{y}) after a {pan} px pan");
                        }
                    }
                }
            }
        }
    }
}
