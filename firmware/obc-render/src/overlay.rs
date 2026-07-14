//! Post-map overlays: the user-position marker, the active route (with its
//! direction chevrons) and the breadcrumb polyline.

use heapless::Vec;

use embedded_graphics::prelude::*;

use crate::fill::fill_polygon;
use crate::stroke::Stroker;
use crate::viewport::round_pt;
use crate::{DrawScratch, MapRenderer, Viewport, MAX_CROSSINGS};

// Route direction chevrons. Anchored to route distance (not screen) so each stays pinned to a
// ground spot, drawn only in a window around the rider. Spacing + window are screen-relative (a
// fixed pixel cadence and a chevron *count*, not ground metres) so chevrons keep an even spread
// across the finest LOD's zoom range; the ground spacing is derived per-frame from the camera's
// m/px. Glyph sizes are screen pixels.

/// On-screen gap between consecutive chevrons (px). Each frame the route-distance spacing is
/// `ARROW_SPACING_PX × m/px`, so chevrons stay evenly spread at any zoom. At the ~0.5 m/px riding
/// zoom this is ≈ 33 m apart on the ground.
const ARROW_SPACING_PX: f32 = 66.0;
/// How many chevrons lead *ahead* of the rider — a count, not a ground distance, so the look-ahead
/// tracks the screen cadence.
const ARROW_AHEAD_COUNT: u32 = 9;
/// How many chevrons trail *behind* the rider. Zero — the breadcrumb shows the travelled line.
const ARROW_BEHIND_COUNT: u32 = 0;
/// Chevron tip reach ahead of its centre (px).
const ARROW_TIP: f32 = 8.0;
/// Chevron base reach behind its centre (px).
const ARROW_BACK: f32 = 2.5;
/// Chevron base half-width (px). Kept under the route's half-stroke so the glyph sits *inside* the
/// line, framed by the route colour whatever map colour the line crosses.
const ARROW_HALF: f32 = 4.5;

/// What the route overlay needs to know about one route chunk.
pub struct OverlayChunk {
    pub bbox: obc_map_scene::BBox,
    /// Cumulative route distance (m) at this chunk's first point.
    pub cum_distance_m: u32,
}

/// The route overlay's view of an active route — implemented by the host over its
/// route reader. Keeps obc-render ignorant of the OBCR format.
pub trait RouteOverlaySource {
    fn chunk_count(&self) -> usize;
    fn chunk(&self, k: usize) -> OverlayChunk;
    fn total_distance_m(&self) -> u32;
    /// Decode chunk `k` and hand its points — `(lon, lat)` microdegrees — to `visit`
    /// as one slice. Implementations own their decode scratch. A failed decode
    /// (flaky SD) simply doesn't call `visit`.
    // The `&mut dyn FnMut(&[…])` spelling *is* the seam (object-safe, alloc-free); a type
    // alias would only hide what implementors must write anyway.
    #[allow(clippy::type_complexity)]
    fn visit_points(&self, k: usize, visit: &mut dyn FnMut(&[(i32, i32)]));
}

impl MapRenderer {
    /// Draw the user-position marker: a chevron at `(lon, lat)` pointing along `course` (degrees CW
    /// from north), or a non-directional diamond when `course` is `None`. Fixed screen-space size.
    /// Call **after** [`render`](MapRenderer::render). Skips drawing when the anchor projects outside
    /// the view (with a small margin). `color` is the already-resolved device color.
    pub fn draw_marker<D>(
        &mut self,
        target: &mut D,
        vp: &Viewport,
        lon: i32,
        lat: i32,
        course: Option<f32>,
        color: D::Color,
    ) where
        D: DrawTarget,
    {
        let (w, h) = (vp.w as i32, vp.h as i32);
        let (sx, sy) = vp.to_screen(lon, lat);
        // Cull when the anchor is well off-screen; a modest margin keeps a just-off-edge marker.
        const MARGIN: i32 = 16;
        if sx < -MARGIN || sx > w + MARGIN || sy < -MARGIN || sy > h + MARGIN {
            return;
        }

        // On-screen "forward" unit vector: project a point a ground step ahead along the course and
        // take the screen delta. Letting the projection do the rotation makes this correct for both
        // north-up and heading-up. The step is sized so integer rounding barely skews the direction;
        // we normalize, so its exact length doesn't matter.
        let forward = course.and_then(|deg| {
            let theta = deg.to_radians();
            let step = (64.0 / vp.zoom).clamp(1.0, 100_000.0);
            let lon2 = lon as f32 + libm::sinf(theta) * step / vp.aspect;
            let lat2 = lat as f32 + libm::cosf(theta) * step;
            let (sx2, sy2) = vp.to_screen(lon2 as i32, lat2 as i32);
            let (dx, dy) = ((sx2 - sx) as f32, (sy2 - sy) as f32);
            let len = libm::sqrtf(dx * dx + dy * dy);
            (len > 1e-3).then(|| (dx / len, dy / len))
        });

        let (cx, cy) = (sx as f32, sy as f32);
        match forward {
            // Chevron: a tip a bit ahead and two base corners swept back and out.
            Some(fwd) => {
                const TIP: f32 = 12.0;
                const BACK: f32 = 6.0;
                const HALF: f32 = 8.0;
                fill_chevron(target, &mut self.draw.xs, (cx, cy), fwd, TIP, BACK, HALF, color, w, h);
            }
            // Stationary glyph: a small orientation-free diamond.
            None => {
                const R: f32 = 7.0;
                let diamond = [round_pt(cx, cy - R), round_pt(cx + R, cy), round_pt(cx, cy + R), round_pt(cx - R, cy)];
                fill_polygon(target, &diamond, &[4], color, w, h, &mut self.draw.xs);
            }
        }
    }

    /// Stroke an active route as a polyline overlay, with optional travel-direction chevrons. Call
    /// **after** [`render`](MapRenderer::render).
    ///
    /// The route arrives through the [`RouteOverlaySource`] seam — chunked `(lon, lat)`
    /// microdegree polylines with per-chunk bbox + cumulative distance — so the renderer never
    /// sees the route file format. Streams chunk-by-chunk: only chunks intersecting the view are
    /// decoded (by the source) and stroked, via [`Stroker`] (view-clipped). Consecutive chunks
    /// share a seam vertex so the strokes join.
    ///
    /// `arrows_at` is the rider's matched route distance (m), or `None` to skip chevrons. When set,
    /// chevrons are drawn in a **second pass** (so they sit on top where the route doubles back)
    /// within a window of [`ARROW_AHEAD_COUNT`] chevrons around that distance.
    ///
    /// Returns `(chunks, points, drawn)`: chunks decoded, points across them (route has no LOD, so
    /// this grows as you zoom out), and vertices *actually* stroked after the view clip + subpixel
    /// simplify (`drawn` ≪ `points` when most of the route is off-screen).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_route<D>(
        &mut self,
        target: &mut D,
        vp: &Viewport,
        route: &dyn RouteOverlaySource,
        color: D::Color,
        weight: u32,
        arrow_color: D::Color,
        arrows_at: Option<u32>,
    ) -> (usize, usize, usize)
    where
        D: DrawTarget,
    {
        let (w, h) = (vp.w as i32, vp.h as i32);
        let view = vp.visible_bbox();
        // Split the borrow so the fills can take `xs` while we build the polyline in `screen`.
        let DrawScratch { screen, xs } = &mut self.draw;
        let (mut route_chunks, mut route_points, mut route_drawn) = (0usize, 0usize, 0usize);

        // Pass 1 — stroke every visible chunk, in full, before any chevron is drawn.
        for k in 0..route.chunk_count() {
            if !route.chunk(k).bbox.intersects(&view) {
                continue;
            }
            route.visit_points(k, &mut |pts| {
                // Adjacent chunks share a seam vertex, counted on both — matching the strokes.
                route_chunks += 1;
                route_points += pts.len();
                let projected = pts.iter().map(|&(lon, lat)| vp.project(lon, lat));
                // Per-chunk `Stroker` (a handful of copies): it must drop before the chevron pass
                // below borrows `xs` on its own.
                route_drawn += Stroker::new(target, screen, color, weight, w, h).stroke(projected);
            });
        }

        // Pass 2 — chevrons, anchored to route distance and windowed around the rider.
        let Some(progress_m) = arrows_at else {
            return (route_chunks, route_points, route_drawn);
        };
        let total = route.total_distance_m();
        // Ground spacing for *this* frame: a fixed screen cadence scaled by m/px (`.max` guards
        // divide-by-zero at absurd zoom-in). The window is then a chevron *count* either side.
        let spacing_m = (ARROW_SPACING_PX * vp.meters_per_pixel()).max(1e-3);
        let lo = (progress_m as f32 - ARROW_BEHIND_COUNT as f32 * spacing_m).max(0.0);
        let hi = (progress_m as f32 + ARROW_AHEAD_COUNT as f32 * spacing_m).min(total as f32);
        for k in 0..route.chunk_count() {
            // Skip chunks whose cumulative-distance span misses the window (then the view).
            let cm = route.chunk(k);
            let chunk_start = cm.cum_distance_m as f32;
            let next_start = if k + 1 < route.chunk_count() { route.chunk(k + 1).cum_distance_m } else { total };
            let chunk_end = next_start as f32;
            if chunk_end < lo || chunk_start > hi || !cm.bbox.intersects(&view) {
                continue;
            }
            route.visit_points(k, &mut |pts| {
                walk_route_arrows(pts, chunk_start, lo, hi, spacing_m, vp.aspect, |a, b, f| {
                    let (ax, ay) = vp.to_screen(a.0, a.1);
                    let (bx, by) = vp.to_screen(b.0, b.1);
                    let (ax, ay, bx, by) = (ax as f32, ay as f32, bx as f32, by as f32);
                    let (dx, dy) = (bx - ax, by - ay);
                    let m = dx.abs().max(dy.abs()) + 0.41 * dx.abs().min(dy.abs());
                    if m < 1e-3 {
                        return;
                    }
                    let fwd = (dx / m, dy / m); // screen travel dir (north-up & heading-up)
                    let centre = (ax + dx * f, ay + dy * f); // chevron centre along the segment
                    fill_chevron(target, xs, centre, fwd, ARROW_TIP, ARROW_BACK, ARROW_HALF, arrow_color, w, h);
                });
            });
        }
        (route_chunks, route_points, route_drawn)
    }

    /// Stroke a single polyline of `(lon, lat)` microdegree points as a view-clipped overlay — the
    /// recorded **breadcrumb**, whose two tiers (spine, recent) are each one call. Call after
    /// [`render`](MapRenderer::render).
    pub fn stroke_path<D, I>(&mut self, target: &mut D, vp: &Viewport, pts: I, color: D::Color, weight: u32)
    where
        D: DrawTarget,
        I: IntoIterator<Item = (i32, i32)>,
    {
        let (w, h) = (vp.w as i32, vp.h as i32);
        let projected = pts.into_iter().map(|(lon, lat)| vp.project(lon, lat));
        // The thick-segment fill scan-converts from a stack edge record, so the stroker needs only
        // `screen`; `xs` stays reserved for the general polygon/chevron fills elsewhere.
        let DrawScratch { screen, .. } = &mut self.draw;
        Stroker::new(target, screen, color, weight, w, h).stroke(projected);
    }
}

/// Walk a decoded route chunk (`(lon, lat)` microdegree points plus `s0`, the cumulative route
/// distance in metres at its first point) and call `emit(a, b, f)` for every chevron whose route
/// distance is a multiple of `spacing_m` inside `[lo, hi]` — `f` is the fraction along segment
/// `a`→`b`. Anchoring to the route's cumulative distance pins each chevron to one ground spot as
/// the camera pans; `[lo, hi]` keeps them near the rider. Segment length is real ground metres;
/// `cl` is the viewport's hoisted `cos(lat)` (computed once per frame), so the walk costs no
/// per-segment `cosf`.
fn walk_route_arrows<F>(pts: &[(i32, i32)], s0: f32, lo: f32, hi: f32, spacing_m: f32, cl: f32, mut emit: F)
where
    F: FnMut((i32, i32), (i32, i32), f32),
{
    let mut s = s0;
    for seg in pts.windows(2) {
        let (a, b) = (seg[0], seg[1]);
        let dl = obc_map_scene::ground_dist_m_cl(a, b, cl);
        if dl > 1e-3 {
            // Grid multiples of spacing_m that fall on this segment and in the window.
            let lo_seg = s.max(lo);
            let hi_seg = (s + dl).min(hi);
            let mut n = libm::ceilf(lo_seg / spacing_m) * spacing_m;
            while n <= hi_seg {
                emit(a, b, ((n - s) / dl).clamp(0.0, 1.0));
                n += spacing_m;
            }
        }
        s += dl;
    }
}

/// Fill a 3-point direction chevron centred at `c`, pointing along the unit vector `fwd`: a tip
/// `tip` px ahead and two base corners swept `back` px behind and `half` px out each side. Shared by
/// the user-position marker and the route arrows; the caller supplies `fwd` already normalized.
#[allow(clippy::too_many_arguments)]
pub(crate) fn fill_chevron<D>(
    target: &mut D,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
    c: (f32, f32),
    fwd: (f32, f32),
    tip: f32,
    back: f32,
    half: f32,
    color: D::Color,
    w: i32,
    h: i32,
) where
    D: DrawTarget,
{
    let (fx, fy) = fwd;
    let (rx, ry) = (-fy, fx); // right perpendicular = base spread
    let tri = [
        round_pt(c.0 + fx * tip, c.1 + fy * tip),
        round_pt(c.0 - fx * back + rx * half, c.1 - fy * back + ry * half),
        round_pt(c.0 - fx * back - rx * half, c.1 - fy * back - ry * half),
    ];
    fill_polygon(target, &tri, &[3], color, w, h, xs);
}

#[cfg(test)]
mod tests {
    use super::walk_route_arrows;
    use crate::viewport::aspect_for_lat;
    use heapless::Vec;
    use obc_map_scene::ground_dist_m;

    /// Fixed spacing (m) to pin the grid maths; the app derives it per-frame from the zoom.
    const SPACING: f32 = 33.0;

    /// A due-north two-point segment ~300 m long (fixed longitude, so length is pure latitude).
    /// Returned with its ground length. Points are `(lon, lat)` microdegrees — the seam's shape.
    fn north_line() -> ([(i32, i32); 2], f32) {
        let v = [(7_800_000, 48_000_000), (7_800_000, 48_002_700)];
        let dl = ground_dist_m(v[0], v[1]);
        (v, dl)
    }

    /// Route distances (m from the segment start) at which chevrons land for a window `[lo,hi]`.
    fn distances(pts: &[(i32, i32)], dl: f32, lo: f32, hi: f32) -> Vec<i32, 64> {
        let mut v = Vec::new();
        let cl = aspect_for_lat(pts[0].1);
        walk_route_arrows(pts, 0.0, lo, hi, SPACING, cl, |_, _, f| {
            let _ = v.push(libm::roundf(f * dl) as i32);
        });
        v
    }

    #[test]
    fn chevrons_land_on_the_spacing_grid() {
        // Chevrons sit at 0, SPACING, 2·SPACING, … of route distance — they're anchored to the
        // route, not the screen, so each is a fixed multiple of the spacing.
        let (pts, dl) = north_line();
        let ds = distances(&pts, dl, 0.0, dl);
        assert!(ds.len() >= 5, "a {dl:.0} m segment should carry several chevrons");
        for (i, d) in ds.iter().enumerate() {
            let expect = libm::roundf(i as f32 * SPACING) as i32;
            assert!((d - expect).abs() <= 1, "chevron {i} at {d} m, expected {expect} m");
        }
    }

    #[test]
    fn chevrons_stay_within_the_window() {
        // Only chevrons inside [lo, hi] are emitted, and a wider window strictly adds more.
        let (pts, dl) = north_line();
        let (lo, hi) = (50.0, 140.0);
        let narrow = distances(&pts, dl, lo, hi);
        assert!(!narrow.is_empty());
        for d in &narrow {
            assert!(*d as f32 >= lo - 0.5 && *d as f32 <= hi + 0.5, "chevron at {d} m outside window");
        }
        assert!(distances(&pts, dl, 0.0, dl).len() > narrow.len());
    }

    #[test]
    fn chevrons_are_pinned_to_route_distance_not_the_rider() {
        // The exact property the redesign is about: slide the window forward (as the rider
        // advances) and the chevrons still visible keep the *same* route distances — they do
        // not crawl with the rider. Here the shared [80, 200] m band must match between a
        // window centred earlier and one centred later.
        let (pts, dl) = north_line();
        let band = |lo, hi| -> Vec<i32, 64> {
            distances(&pts, dl, lo, hi).iter().copied().filter(|&d| (80..=200).contains(&d)).collect()
        };
        let early = band(0.0, 210.0);
        let late = band(70.0, 280.0);
        assert!(!early.is_empty(), "the shared band should contain chevrons");
        assert_eq!(early, late, "a chevron moved when the window slid — it should be ground-pinned");
    }
}
