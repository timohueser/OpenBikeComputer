//! Scanline even-odd polygon fill.

use heapless::Vec;

use embedded_graphics::{prelude::*, primitives::Rectangle};

use crate::viewport::Viewport;
use crate::{MAX_CROSSINGS, MAX_DECODE_RINGS, MAX_SCREEN_POINTS};

/// Project a feature's microdegree rings into `screen` and scanline-fill them. The draw phase's
/// `Kind::Polygon` arm; also the marker diamond's path.
pub(crate) fn fill_polygon_proj<D>(
    target: &mut D,
    vp: &Viewport,
    pts: &[(i32, i32)],
    ring_lens: &[usize],
    color: D::Color,
    screen: &mut Vec<Point, MAX_SCREEN_POINTS>,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
{
    screen.clear();
    for &(lon, lat) in pts {
        let _ = screen.push(vp.project(lon, lat));
    }
    fill_polygon(target, screen, ring_lens, color, vp.w as i32, vp.h as i32, xs);
}

/// Scanline even-odd polygon fill. `screen` holds every ring's projected points concatenated;
/// `ring_lens` partitions them (exterior first, then holes — holes fall out of the even-odd rule
/// for free). A row overflowing `xs` is skipped to keep even-odd parity intact rather than pairing
/// spans from a truncated crossing list.
pub(crate) fn fill_polygon<D>(
    target: &mut D,
    screen: &[Point],
    ring_lens: &[usize],
    color: D::Color,
    w: i32,
    h: i32,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
{
    let mut ymin = i32::MAX;
    let mut ymax = i32::MIN;
    for p in screen {
        ymin = ymin.min(p.y);
        ymax = ymax.max(p.y);
    }
    ymin = ymin.max(0);
    ymax = ymax.min(h - 1);
    if ymin > ymax {
        return;
    }
    // Per-ring y-ranges, hoisted out of the row loop so a scanline outside a ring's band skips it
    // without touching its edges — the whole-polygon `ymin/ymax` above only bounds the union, so
    // this is what saves work on multi-ring features and tall-skinny-ring layouts. Sized to
    // `MAX_DECODE_RINGS` (the decode path's cap); the fixed tiny arrays other callers pass always
    // fit, but if a ring doesn't (overflow), it simply isn't culled — today's always-test behavior.
    let mut ring_y: Vec<(i32, i32), MAX_DECODE_RINGS> = Vec::new();
    {
        let mut base = 0usize;
        for &len in ring_lens {
            let mut ry_min = i32::MAX;
            let mut ry_max = i32::MIN;
            for p in &screen[base..base + len] {
                ry_min = ry_min.min(p.y);
                ry_max = ry_max.max(p.y);
            }
            base += len;
            if ring_y.push((ry_min, ry_max)).is_err() {
                break;
            }
        }
    }
    for y in ymin..=ymax {
        let yc = y as f32 + 0.5;
        xs.clear();
        let mut base = 0usize;
        let mut saturated = false;
        'rings: for (r, &len) in ring_lens.iter().enumerate() {
            let ring = &screen[base..base + len];
            base += len;
            if len < 2 {
                continue;
            }
            // Rows outside the ring's y-band can't cross it; rings past the (unreachable in the
            // decode path) `ring_y` capacity fall back to the full edge test.
            if let Some(&(ry_min, ry_max)) = ring_y.get(r) {
                if yc < ry_min as f32 || yc > ry_max as f32 {
                    continue;
                }
            }
            let mut j = len - 1;
            for i in 0..len {
                let (xi, yi) = (ring[i].x as f32, ring[i].y as f32);
                let (xj, yj) = (ring[j].x as f32, ring[j].y as f32);
                if (yi <= yc && yc < yj) || (yj <= yc && yc < yi) {
                    // A row crossing the outline more than MAX_CROSSINGS times can't be captured
                    // whole; pairing a truncated list would break even-odd parity and paint
                    // background-colored gaps. Skip the row instead — an unfilled 1px seam on the
                    // densest features beats a mis-filled span, and the buffer can't grow without
                    // busting the MCU_SCRATCH_BYTES budget.
                    if xs.push(xi + (yc - yi) / (yj - yi) * (xj - xi)).is_err() {
                        saturated = true;
                        break 'rings;
                    }
                }
                j = i;
            }
        }
        if saturated || xs.len() < 2 {
            continue;
        }
        xs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let mut k = 0;
        while k + 1 < xs.len() {
            // Spans round *outward* — see [`fill_span`], which owns that rule for both fillers.
            // A feature clipped across a chunk boundary becomes two polygons whose shared edge is
            // clipped independently, so their pixel staircases can disagree by ≤1px (most visible
            // along a rotated diagonal seam). `to_screen`'s round-to-nearest collapses nearly all of
            // it; the ≤1px overlap is cheap insurance (invisible for same-colored fills).
            fill_span(target, xs[k], xs[k + 1], y, w, color);
            k += 2;
        }
    }
}

/// Emit one outward-rounded solid span for row `y` covering `left..=right` in sub-pixel x — the
/// shared span-emit of [`fill_polygon`] and [`fill_convex_quad`]. Rounds **outward** (`floor` the
/// left edge, `ceil` the right) and clamps to `0..=w-1`, so adjacent fills overlap by ≤1 px rather
/// than leaving a hairline crack (see [`fill_polygon`]). A span that clips to nothing draws nothing.
#[inline]
fn fill_span<D>(target: &mut D, left: f32, right: f32, y: i32, w: i32, color: D::Color)
where
    D: DrawTarget,
{
    let x0 = (libm::floorf(left) as i32).max(0);
    let x1 = (libm::ceilf(right) as i32).min(w - 1);
    if x1 >= x0 {
        let _ = target.fill_solid(&Rectangle::new(Point::new(x0, y), Size::new((x1 - x0 + 1) as u32, 1)), color);
    }
}

/// Scan-convert a **convex four-point quad** — a thick stroke segment's swept rectangle — as one
/// solid span per row. The specialization of [`fill_polygon`] for the `fill_butt_quad`
/// (`crate::stroke`) call site: a convex quad crosses any scanline exactly twice, so the generic
/// filler's crossing-vector clear/push/overflow-guard, its per-row two-element sort, and its
/// per-ring y-band bookkeeping are all avoidable — we keep the minimum and maximum active x
/// directly and emit a single rectangle.
///
/// **Byte-identical to [`fill_polygon`] on the same quad.** This is a hard requirement, not just a
/// scene-hash coincidence, so the per-row crossing is computed with the *exact same expression*
/// [`fill_polygon`] uses — base vertex `i`, previous vertex `j`, `xi + (yc - yi)/(yj - yi)*(xj -
/// xi)` — rather than a hoisted reciprocal slope, whose different float rounding drifts spans by a
/// pixel on some quads (the differential harness in `crate::stroke` catches it). The half-open edge
/// rule (`y_min <= yc < y_max`) and the outward span rounding ([`fill_span`]) are likewise the same
/// pixel contract. The saved work is the scratch buffer, the sort, and the ring machinery — not the
/// division.
///
/// `round_pt` can collapse a short or shallow segment's quad to a triangle or a zero-area sliver;
/// those still cross any row at most twice, so the fast path holds. Should a rounded quad ever
/// present **more than two** crossings on a row (a self-intersecting or concave degenerate), that
/// row falls through to the very same even-odd sort-and-pair [`fill_polygon`] runs — so the output
/// can never drift from the general filler regardless of what geometry `round_pt` produces.
///
/// Fixed-size and allocation-free: at most four stack edge records (~64 bytes) — no resident buffer,
/// no [`crate::DrawScratch`]. `#[inline(never)]` keeps these locals off the already-deep stroker
/// frame.
#[inline(never)]
pub(crate) fn fill_convex_quad<D>(target: &mut D, quad: &[Point; 4], color: D::Color, w: i32, h: i32)
where
    D: DrawTarget,
{
    /// One non-horizontal quad side, in [`fill_polygon`]'s `i`/`j` roles (`i` = current vertex, `j`
    /// = previous): `xi`/`yi` are vertex `i`, `dx`/`dy` are `xj - xi`/`yj - yi`. The half-open
    /// active test `y_min <= yc < y_max` is exactly `(yi<=yc<yj) || (yj<=yc<yi)`, so it drops
    /// horizontal sides for free (they can never satisfy it) — those are never recorded.
    struct Edge {
        y_min: f32,
        y_max: f32,
        xi: f32,
        yi: f32,
        dx: f32,
        dy: f32,
    }

    // Union y-range clamped to the framebuffer, exactly as `fill_polygon`.
    let mut ymin = i32::MAX;
    let mut ymax = i32::MIN;
    for p in quad {
        ymin = ymin.min(p.y);
        ymax = ymax.max(p.y);
    }
    ymin = ymin.max(0);
    ymax = ymax.min(h - 1);
    if ymin > ymax {
        return;
    }

    // Build the (≤4) non-horizontal edges once, iterating `i`/`j` as `fill_polygon` does (i = the
    // current vertex, j = its predecessor, starting j = quad[3]).
    let mut edges: Vec<Edge, 4> = Vec::new();
    let mut prev = quad[3];
    for &cur in quad {
        if prev.y != cur.y {
            let (xi, yi) = (cur.x as f32, cur.y as f32);
            let (xj, yj) = (prev.x as f32, prev.y as f32);
            let (y_min, y_max) = if yi < yj { (yi, yj) } else { (yj, yi) };
            let _ = edges.push(Edge { y_min, y_max, xi, yi, dx: xj - xi, dy: yj - yi });
        }
        prev = cur;
    }

    for y in ymin..=ymax {
        let yc = y as f32 + 0.5;
        // Keep the min/max active x directly — no crossing buffer, no sort — plus a tiny fixed
        // record of every crossing so a >2-crossing degenerate row can fall back to exact even-odd.
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        let mut xs4 = [0.0f32; 4];
        let mut n = 0usize;
        for e in &edges {
            if e.y_min <= yc && yc < e.y_max {
                // Bit-for-bit `fill_polygon`'s crossing: `xi + (yc - yi)/(yj - yi)*(xj - xi)`.
                let x = e.xi + (yc - e.yi) / e.dy * e.dx;
                xs4[n] = x;
                n += 1;
                if x < lo {
                    lo = x;
                }
                if x > hi {
                    hi = x;
                }
            }
        }
        if n < 2 {
            continue; // matches `fill_polygon`'s `xs.len() < 2` row skip
        }
        if n == 2 {
            fill_span(target, lo, hi, y, w, color); // the fast path: sort of two is just (min, max)
        } else {
            // A rounded quad that presents 3–4 crossings on this row is non-convex/self-intersecting.
            // Mirror `fill_polygon` exactly — sort the crossings and pair them even-odd — so the
            // specialized filler can never diverge from the general one on any reachable geometry.
            let s = &mut xs4[..n];
            s.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
            let mut k = 0;
            while k + 1 < n {
                fill_span(target, s[k], s[k + 1], y, w, color);
                k += 2;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fill_polygon;
    use crate::MAX_CROSSINGS;
    use heapless::Vec;

    #[test]
    fn fill_polygon_skips_rows_that_overflow_the_crossing_buffer() {
        // A scanline crossing the outline more than MAX_CROSSINGS times must be skipped, not filled
        // from the truncated crossing list (which corrupts even-odd parity), while ordinary rows of
        // the same polygon still fill correctly.
        use embedded_graphics::{pixelcolor::BinaryColor, prelude::*, primitives::Rectangle};

        const P: usize = 200; // prongs → 2·P scanline crossings in the prong band
        const W: i32 = 2 * P as i32; // one column per prong + its gap
        const H: i32 = 8;
        const HBASE: i32 = 4; // prongs span y ∈ [0, HBASE); a solid base sits below
        const HBOTTOM: i32 = 6;
        // The comb only proves anything if it actually overflows the buffer.
        const { assert!(2 * P > MAX_CROSSINGS, "comb must exceed MAX_CROSSINGS to exercise saturation") };

        // Records pixels painted per row via fill_solid, so a skipped row (0) is distinguishable
        // from a correctly filled one (full width).
        struct RowFill {
            rows: [u32; H as usize],
        }
        impl OriginDimensions for RowFill {
            fn size(&self) -> Size {
                Size::new(W as u32, H as u32)
            }
        }
        impl DrawTarget for RowFill {
            type Color = BinaryColor;
            type Error = core::convert::Infallible;
            fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
            where
                I: IntoIterator<Item = Pixel<Self::Color>>,
            {
                for Pixel(p, _) in pixels {
                    if (0..H).contains(&p.y) && (0..W).contains(&p.x) {
                        self.rows[p.y as usize] += 1;
                    }
                }
                Ok(())
            }
            fn fill_solid(&mut self, area: &Rectangle, _: Self::Color) -> Result<(), Self::Error> {
                let y = area.top_left.y;
                if (0..H).contains(&y) {
                    self.rows[y as usize] += area.size.width;
                }
                Ok(())
            }
        }

        // A comb: P vertical 1px prongs (1px gaps) standing on a solid base. A
        // scanline through the prongs crosses both walls of every prong (2·P);
        // one through the base crosses only the two outer walls.
        let mut poly: Vec<Point, 1024> = Vec::new();
        poly.push(Point::new(0, 0)).unwrap();
        for i in 0..P as i32 {
            let x1 = 2 * i + 1;
            poly.push(Point::new(x1, 0)).unwrap(); // prong top-right
            poly.push(Point::new(x1, HBASE)).unwrap(); // right wall down to base
            if i + 1 < P as i32 {
                poly.push(Point::new(x1 + 1, HBASE)).unwrap(); // base across the gap
                poly.push(Point::new(x1 + 1, 0)).unwrap(); // next prong's left wall up
            }
        }
        poly.push(Point::new(W - 1, HBOTTOM)).unwrap(); // right wall down past the base
        poly.push(Point::new(0, HBOTTOM)).unwrap(); // base bottom edge (closing edge → (0,0))

        let mut target = RowFill { rows: [0; H as usize] };
        let mut xs: Vec<f32, MAX_CROSSINGS> = Vec::new();
        let len = poly.len();
        fill_polygon(&mut target, &poly, &[len], BinaryColor::On, W, H, &mut xs);

        // Prong-band rows overflow the buffer → skipped, not mis-filled.
        for y in 0..HBASE {
            assert_eq!(target.rows[y as usize], 0, "saturated prong row {y} must be left unfilled, not mis-filled");
        }
        // Base-band rows have just two crossings → filled edge to edge.
        for y in HBASE..HBOTTOM {
            assert_eq!(target.rows[y as usize], W as u32, "base row {y} should fill the full width");
        }
    }
}
