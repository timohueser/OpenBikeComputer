//! Polyline stroking: view clip, subpixel simplify, and the thick-stroke rasteriser.

use heapless::Vec;

use embedded_graphics::{
    prelude::*,
    primitives::{Polyline, PrimitiveStyle, Rectangle},
};

use crate::fill::fill_polygon;
use crate::viewport::{round_pt, Viewport};
use crate::{MAX_CROSSINGS, MAX_SCREEN_POINTS};

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

/// The `cos²θ` threshold below which a `weight`-px thick stroke's bare butt-join is within ½ px of
/// a round joint, so the vertex needs no round-join disc in [`flush_run`]. Butt ends meet at the
/// vertex; on the outer side of a turn `θ` that leaves a notch ~`r·sin(θ/2)` deep (`r = weight/2`).
/// Sub-pixel means `sin(θ/2) ≤ 1/weight`, so the cut-off cosine is `1 − 2·(1/weight)²` — returned
/// squared for the magnitude-folded test. At weight 11 that's a ~10° cut-off.
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

/// Screen-space simplification tolerance (px) for [`Stroker::stroke`]. **Subpixel** by design: big
/// enough to fold away the integer-projection staircase (≤ ½ px) and same-pixel vertex pile-ups,
/// but under 1 px so the stroked line never shifts a visible pixel.
const SIMPLIFY_EPS_PX: f32 = 0.75;

/// True when `p` lies within `eps` px (perpendicular) of the line through `a` and `b` — the
/// near-collinear test [`simplify`] uses. Cross / length-squared in `f32` (no `sqrt`); degenerate
/// `a == b` falls back to `|p − a|`.
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

/// Streaming one-lookahead collinear simplification: calls `emit` for the first vertex, the last,
/// and every vertex bending off the line through its kept neighbours by more than `eps` px
/// ([`within_eps`]). O(1) state; each dropped vertex lies within `eps` of the kept path.
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

/// One stroke operation's invariants + scratch, borrowed for the duration of a single polyline
/// stroke. `run` accumulates the current visible segment run; `xs` is the scanline-crossing
/// scratch the span fills use. Bundling these keeps the pipeline's helpers argument-light and
/// gives the coming per-stroke state (v6 dashes, road casing) a home.
pub(crate) struct Stroker<'a, D: DrawTarget> {
    target: &'a mut D,
    run: &'a mut Vec<Point, MAX_SCREEN_POINTS>,
    xs: &'a mut Vec<f32, MAX_CROSSINGS>,
    color: D::Color,
    /// Stroke width in px, already `.max(1)`-clamped by [`Stroker::new`].
    weight: u32,
    /// When set ([`Stroker::stroke_dashed`]), each flushed run rasterises as screen-space dashes
    /// instead of a solid stroke. Off by default, so [`Stroker::stroke`] is byte-for-byte unchanged.
    dashed: bool,
    /// View rectangle grown by the stroke width ([`Stroker::new`]), as `(xmin, ymin, xmax, ymax)`.
    clip: (f32, f32, f32, f32),
    w: i32,
    h: i32,
}

impl<'a, D: DrawTarget> Stroker<'a, D> {
    /// Borrow the target + scratch and fix this stroke's invariants: `weight` clamped to ≥ 1 and
    /// the clip rectangle grown by the stroke width, so an edge-hugging line keeps its full
    /// thickness. Clears `run` — the accumulator must start empty for [`Stroker::stroke`].
    pub(crate) fn new(
        target: &'a mut D,
        run: &'a mut Vec<Point, MAX_SCREEN_POINTS>,
        xs: &'a mut Vec<f32, MAX_CROSSINGS>,
        color: D::Color,
        weight: u32,
        w: i32,
        h: i32,
    ) -> Self {
        let weight = weight.max(1);
        let m = weight as f32 + 2.0; // clip margin ≥ half-width, so edge strokes still paint in
        let clip = (-m, -m, w as f32 + m, h as f32 + m);
        run.clear();
        Self { target, run, xs, color, weight, dashed: false, clip, w, h }
    }

    /// Clip a projected overlay polyline to the view and stroke the on-screen runs
    /// ([`Stroker::flush_run`]). Clipping first (Cohen–Sutherland, into the grown clip rectangle)
    /// means the stroker only pays for the visible part — vital when the route/breadcrumb is ~96%
    /// off-screen at riding zoom. The line splits into separate runs where it crosses the view,
    /// each stroked on its own.
    ///
    /// Points are first **simplified in screen space** ([`simplify`] at [`SIMPLIFY_EPS_PX`]) — a
    /// subpixel dedup folding away the integer-projection staircase and same-pixel pile-ups without
    /// moving the line a visible pixel, handing the stroker far fewer segments and joints.
    ///
    /// Returns the count of **on-screen vertices actually stroked** (after simplify + view clip).
    pub(crate) fn stroke<I>(&mut self, points: I) -> usize
    where
        I: IntoIterator<Item = Point>,
    {
        // Consecutive kept vertices stroke as clipped segments — runs join because each segment
        // starts where the previous ended.
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

    /// Like [`Stroker::stroke`] but rasterises the on-screen runs as **dashes** ([`walk_dashes`]):
    /// the whole simplify → clip → run-accumulation pipeline is reused unchanged (so off-screen
    /// dashes cost nothing — clip-before-dash), and only [`Stroker::flush_run`] diverges once the
    /// `dashed` flag is set. Dash phase resets at each run (each clip re-entry), the accepted v1
    /// "crawl" during pans (epic #556).
    pub(crate) fn stroke_dashed<I>(&mut self, points: I) -> usize
    where
        I: IntoIterator<Item = Point>,
    {
        self.dashed = true;
        self.stroke(points)
    }

    /// Clip one committed segment `a`→`b` to the view and append it to the current run, flushing
    /// where the line is discontinuous (segment off-screen, or it doesn't continue the last run).
    ///
    /// Returns how many **on-screen vertices** this segment contributed (0 if wholly clipped out)
    /// — `c1` always, plus `c0` when it (re)starts a run.
    fn stroke_seg(&mut self, a: Point, b: Point) -> usize {
        let (xmin, ymin, xmax, ymax) = self.clip;
        match clip_segment(a, b, xmin, ymin, xmax, ymax) {
            None => {
                self.flush_run(); // segment wholly off-screen
                0
            }
            Some((c0, c1)) => {
                let mut drawn = 1; // c1
                                   // (Re)start a run if this segment didn't continue the previous one.
                if self.run.last().copied() != Some(c0) {
                    self.flush_run();
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
        }
    }

    /// Rasterise the accumulated run, then clear it for the next.
    ///
    /// A **1 px** stroke goes through embedded-graphics' `Polyline` — a thin Bresenham line, and
    /// the one width the span path can't do (a zero-width rectangle has no scanline crossings).
    /// **Everything ≥ 2 px** is laid down as **spans**: a filled rectangle per segment
    /// ([`Stroker::fill_thick_segment`]) plus a round-join/cap disc ([`Stroker::fill_disc`]) at the
    /// two run ends (always — they round the cap and, at a chunk seam, close the butt gap to the
    /// next feature) and at every interior vertex bending sharply enough to show a notch
    /// ([`turn_is_sharp`]). Both go through the coalesced `fill_solid`. eg's thick `Polyline` +
    /// `Circle` path measured ~10× a span stroke even at 2 px, so the split sits at 1 px, not 2.
    fn flush_run(&mut self) {
        if self.run.len() >= 2 {
            if self.dashed {
                self.flush_run_dashed();
            } else if self.weight <= 1 {
                let _ = Polyline::new(self.run)
                    .into_styled(PrimitiveStyle::with_stroke(self.color, self.weight))
                    .draw(self.target);
            } else {
                // Body half-width is the integer disc radius, not `weight/2` — so rectangle and disc come
                // out the same thickness (disc never narrower than the body it caps), and an odd `weight`
                // lands on its nominal width instead of a px fatter.
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
        self.run.clear();
    }

    /// Rasterise the accumulated run as **screen-space dashes** in `self.color` ([`walk_dashes`],
    /// `on == off == `[`dash_len`]). Reuses everything up to here unchanged (simplify → clip → run),
    /// so a dashed line is *cheaper* than a solid one — off-screen dashes are already clipped away
    /// and no run is walked twice. The on-intervals emit with **butt ends, no joint/cap discs**:
    /// dashes are short and straight, so the notch a disc would fill is invisible, and skipping them
    /// halves the fill cost and keeps the stripe edges crisp. Called only from [`Stroker::flush_run`]
    /// (run length already `>= 2`); `flush_run` clears the run afterwards.
    fn flush_run_dashed(&mut self) {
        let dash = dash_len(self.weight);
        let hw = (self.weight / 2) as f32;
        let weight = self.weight;
        // Reborrow disjoint fields so `walk_dashes` may read `run` while the emit closure writes the
        // target/scratch — the closure captures locals, never `self`.
        let target = &mut *self.target;
        let xs = &mut *self.xs;
        let color = self.color;
        let (w, h) = (self.w, self.h);
        walk_dashes(self.run, dash, |a, b| {
            if a == b {
                return; // an on-interval that rounded onto a single pixel
            }
            if weight <= 1 {
                let _ = Polyline::new(&[a, b]).into_styled(PrimitiveStyle::with_stroke(color, weight)).draw(target);
            } else {
                fill_butt_quad(target, a, b, hw, color, w, h, xs);
            }
        });
    }

    /// Lay down one segment of a thick stroke as a filled rectangle (swept ±`hw` px along its
    /// perpendicular). Thin wrapper over [`fill_butt_quad`], sharing the quad math with the dash
    /// path. A zero-length segment is left to the joint/cap disc.
    fn fill_thick_segment(&mut self, a: Point, b: Point, hw: f32) {
        fill_butt_quad(self.target, a, b, hw, self.color, self.w, self.h, self.xs);
    }

    /// Fill a solid disc of radius `r` px at `(cx, cy)` as horizontal spans — one
    /// [`fill_solid`](DrawTarget::fill_solid) per row (`hw = √(r² − dy²)`), not embedded-graphics'
    /// per-pixel `Circle`. Rounds the thick stroke's joints and caps. Rows off top/bottom skipped;
    /// `fill_solid` clips x.
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

/// Lay down one thick-stroke segment as a filled rectangle (swept ±`hw` px along its perpendicular)
/// via [`fill_polygon`] — a convex quad, so every row has exactly two crossings. **Butt ends** (no
/// end caps): both the solid stroke's per-segment fill ([`Stroker::fill_thick_segment`], which caps
/// via separate joint discs) and the dash path reuse this. A zero-length segment draws nothing.
/// Spans round **outward** (see `fill_polygon`), so adjacent quads overlap by ≤1 px, no hairline
/// crack.
#[allow(clippy::too_many_arguments)]
fn fill_butt_quad<D>(
    target: &mut D,
    a: Point,
    b: Point,
    hw: f32,
    color: D::Color,
    w: i32,
    h: i32,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
{
    let (ax, ay, bx, by) = (a.x as f32, a.y as f32, b.x as f32, b.y as f32);
    let (dx, dy) = (bx - ax, by - ay);
    let len = libm::sqrtf(dx * dx + dy * dy);
    if len < 1e-3 {
        return;
    }
    let (nx, ny) = (-dy / len * hw, dx / len * hw); // perpendicular × half-width
    let quad = [
        round_pt(ax + nx, ay + ny),
        round_pt(bx + nx, by + ny),
        round_pt(bx - nx, by - ny),
        round_pt(ax - nx, ay - ny),
    ];
    fill_polygon(target, &quad, &[4], color, w, h, xs);
}

/// Dash on/off length in screen px for a `weight`-px dashed stroke (`on == off`). **Screen-space and
/// zoom-independent** — a locked epic decision, no per-style config knob — so the dash rhythm reads
/// the same at every zoom. Scales gently with weight (thicker stripe ⇒ longer dashes) and clamps to
/// a legible 4–12 px. The exact numbers are a by-eye call; tune here, see epic #556.
fn dash_len(weight: u32) -> f32 {
    (3 * weight).clamp(4, 12) as f32
}

/// Walk an already-clipped, screen-space polyline `run` and emit each **"on" dash interval** as a
/// `(start, end)` point pair to `emit`, using `on == off == dash` px of **arc length**. The phase
/// accumulates across the run's segments (so dashes read continuously through a bend) and starts at
/// 0 — i.e. it resets per run, per clip re-entry ([`Stroker::stroke_dashed`]). An on-interval that
/// spans a vertex is emitted as two pieces (one per segment); the pure arc-length math lives here so
/// it can be unit-tested apart from any draw target.
fn walk_dashes<F>(run: &[Point], dash: f32, mut emit: F)
where
    F: FnMut(Point, Point),
{
    let period = 2.0 * dash;
    let mut phase = 0.0_f32; // arc position within [0, period); carries across segments
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
            // Distance to the next on/off boundary, clamped to what's left of the segment. Always
            // > 0 (phase ∈ [0, period), so both `dash - phase` and `period - phase` are positive),
            // so `t` strictly advances — no stall.
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

/// Project and stroke one map line (its exterior ring) — the draw phase's `Kind::Line` arm, and the
/// single point where per-feature line styling branches on the resolved [`Style`](obc_reader::Style)
/// (`dashed` + optional device-quantized `color2`):
///
/// - **solid, no `color2`** → today's single stroke, byte-for-byte unchanged (the zero-cost path).
/// - **solid + `color2`** → still a single solid stroke here; casing is a separate finest-LOD pass
///   (#559), so `draw_line` itself never double-strokes for casing.
/// - **dashed, no `color2`** → dashes in `color`, transparent gaps (admin borders).
/// - **dashed + `color2`** → **railway stripe**: a full solid base in `color2`, then dashes in
///   `color` on top → alternating segments. Re-projects for the second pass (cheap vs. buffering).
///
/// Uses the same view-clipped stroke as the route/breadcrumb overlays.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_line<D>(
    target: &mut D,
    vp: &Viewport,
    pts: &[(i32, i32)],
    color: D::Color,
    weight: u32,
    dashed: bool,
    color2: Option<D::Color>,
    screen: &mut Vec<Point, MAX_SCREEN_POINTS>,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
{
    let (w, h) = (vp.w as i32, vp.h as i32);
    match (dashed, color2) {
        // Solid (with or without color2 — casing is #559's job, not draw_line's). Unchanged path.
        (false, _) => {
            let projected = pts.iter().map(|&(lon, lat)| vp.project(lon, lat));
            Stroker::new(target, screen, xs, color, weight, w, h).stroke(projected);
        }
        (true, None) => {
            let projected = pts.iter().map(|&(lon, lat)| vp.project(lon, lat));
            Stroker::new(target, screen, xs, color, weight, w, h).stroke_dashed(projected);
        }
        (true, Some(c2)) => {
            let base = pts.iter().map(|&(lon, lat)| vp.project(lon, lat));
            Stroker::new(target, screen, xs, c2, weight, w, h).stroke(base);
            let dashes = pts.iter().map(|&(lon, lat)| vp.project(lon, lat));
            Stroker::new(target, screen, xs, color, weight, w, h).stroke_dashed(dashes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{dash_len, joint_disc_cos2, simplify, turn_is_sharp, walk_dashes, within_eps};
    use embedded_graphics::prelude::Point;
    use heapless::Vec;

    /// Collect the vertices [`simplify`] keeps from `pts` at tolerance `eps`.
    fn kept(pts: &[Point], eps: f32) -> Vec<Point, 64> {
        let mut out = Vec::new();
        simplify(pts.iter().copied(), eps, |p| {
            let _ = out.push(p);
        });
        out
    }

    /// Collect the on-dash intervals [`walk_dashes`] emits for `run` at on/off length `dash`.
    fn dashes(run: &[Point], dash: f32) -> Vec<(Point, Point), 64> {
        let mut out = Vec::new();
        walk_dashes(run, dash, |a, b| {
            let _ = out.push((a, b));
        });
        out
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
        // y = round(0.4·x): a straight line the integer projection turned into a staircase. Every
        // point sits within ½ px of the true line, so a subpixel tolerance drops all but the ends.
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

    #[test]
    fn walk_dashes_alternates_on_off_from_the_run_start() {
        // A straight 20 px run at dash 4 ⇒ period 8: on [0,4], off, on [8,12], off, on [16,20].
        // The run always *starts* with an "on" dash (phase resets to 0 at the clip entry point).
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
    fn walk_dashes_resets_phase_per_call() {
        // "Reset per run" = the walker is stateless across calls: a run that doesn't start at the
        // origin still opens with a dash at its first point (the clip re-entry always paints).
        let out = dashes(&[Point::new(5, 5), Point::new(9, 5)], 4.0);
        assert_eq!(&out[..], &[(Point::new(5, 5), Point::new(9, 5))], "one full dash from the entry point");
    }

    #[test]
    fn walk_dashes_carries_phase_across_a_vertex() {
        // An L-bend where a single "on" dash straddles the corner: arc 0..4 with a vertex at arc 3.
        // It must split into a 3 px piece on the first arm and a 1 px piece on the second — meeting
        // at the vertex — with the *off* gap that follows continuing seamlessly on the second arm.
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
        // A repeated vertex (a zero-length segment) leaves the phase untouched — the dash rhythm is
        // identical to the same run without the duplicate point (clip/simplify can hand us these).
        let with_dup = dashes(&[Point::new(0, 0), Point::new(10, 0), Point::new(10, 0), Point::new(20, 0)], 4.0);
        let without = dashes(&[Point::new(0, 0), Point::new(10, 0), Point::new(20, 0)], 4.0);
        assert_eq!(&with_dup[..], &without[..]);
    }

    #[test]
    fn simplify_keeps_a_real_corner() {
        // A right-angle L: the straight arms collapse, but the corner bends far past any subpixel
        // tolerance, so it survives — shape is preserved, only redundant vertices go.
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
}
