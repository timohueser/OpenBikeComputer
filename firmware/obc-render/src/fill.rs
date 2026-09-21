//! Scanline even-odd polygon fill.

use heapless::Vec;

use embedded_graphics::{prelude::*, primitives::Rectangle};

use crate::{collect::ScreenPoint, MAX_CROSSINGS, MAX_DECODE_RINGS, MAX_SCREEN_POINTS};

#[cfg(test)]
use crate::viewport::Viewport;
/// One non-horizontal polygon edge in the exact `i`/previous-`j` orientation [`fill_polygon`]
/// uses. Four packed panel coordinates keep a whole feature's edge table in the same phase-shared
/// backing as the point buffer, so no arena bytes are added.
#[derive(Clone, Copy)]
#[repr(C)]
pub(crate) struct PackedEdge {
    xi: i16,
    yi: i16,
    xj: i16,
    yj: i16,
}

const _: () = assert!(core::mem::size_of::<PackedEdge>() == core::mem::size_of::<Point>());
const _: () = assert!(core::mem::align_of::<PackedEdge>() <= core::mem::align_of::<Point>());

/// Scan-convert retained screen-space rings after building their non-horizontal edges once, which
/// takes ring partitioning, horizontal-edge rejection and point loads out of every scanline while
/// keeping the crossing expression and the half-open edge rule bit for bit.
pub(crate) fn fill_polygon_edges<D, L>(
    target: &mut D,
    points: &[ScreenPoint],
    ring_lens: &[L],
    color: D::Color,
    (w, h): (i32, i32),
    edges: &mut Vec<PackedEdge, MAX_SCREEN_POINTS>,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
    L: Copy + Into<usize>,
{
    edges.clear();
    let mut ymin = i32::MAX;
    let mut ymax = i32::MIN;
    let mut base = 0usize;
    for &len in ring_lens {
        let len = len.into();
        let ring = &points[base..base + len];
        base += len;
        if ring.len() < 2 {
            continue;
        }
        let mut previous = ring[ring.len() - 1].tuple();
        for point in ring {
            let current = point.tuple();
            ymin = ymin.min(current.1);
            ymax = ymax.max(current.1);
            if current.1 != previous.1 {
                // A retained feature cannot exceed MAX_SCREEN_POINTS, so removing horizontal
                // edges makes this push infallible.
                let _ = edges.push(PackedEdge {
                    xi: current.0 as i16,
                    yi: current.1 as i16,
                    xj: previous.0 as i16,
                    yj: previous.1 as i16,
                });
            }
            previous = current;
        }
    }
    ymin = ymin.max(0);
    ymax = ymax.min(h - 1);
    if ymin > ymax {
        return;
    }

    for y in ymin..=ymax {
        let yc = y as f32 + 0.5;
        xs.clear();
        let mut saturated = false;
        for edge in edges.iter() {
            let (xi, yi) = (edge.xi as f32, edge.yi as f32);
            let (xj, yj) = (edge.xj as f32, edge.yj as f32);
            if ((yi <= yc && yc < yj) || (yj <= yc && yc < yi))
                && xs.push(xi + (yc - yi) / (yj - yi) * (xj - xi)).is_err()
            {
                saturated = true;
                break;
            }
        }
        if saturated || xs.len() < 2 {
            continue;
        }
        xs.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());
        let mut k = 0;
        while k + 1 < xs.len() {
            fill_span(target, xs[k], xs[k + 1], y, w, color);
            k += 2;
        }
    }
}

/// Project a feature's microdegree rings into `screen` and scanline-fill them. Retained as the
/// framebuffer-equivalence oracle for the collector's screen-space compaction tests.
#[cfg(test)]
pub(crate) fn fill_polygon_proj<D, L>(
    target: &mut D,
    vp: &Viewport,
    pts: &[(i32, i32)],
    ring_lens: &[L],
    color: D::Color,
    screen: &mut Vec<Point, MAX_SCREEN_POINTS>,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
    L: Copy + Into<usize>,
{
    screen.clear();
    for &(lon, lat) in pts {
        let _ = screen.push(vp.project(lon, lat));
    }
    fill_polygon(target, screen, ring_lens, color, vp.w as i32, vp.h as i32, xs);
}

/// Scanline even-odd polygon fill. `screen` holds every ring's projected points concatenated and
/// `ring_lens` partitions them, exterior first, so holes fall out of the even-odd rule for free. A
/// row overflowing `xs` is skipped, to keep even-odd parity rather than pair spans from a
/// truncated crossing list.
pub(crate) fn fill_polygon<D, L>(
    target: &mut D,
    screen: &[Point],
    ring_lens: &[L],
    color: D::Color,
    w: i32,
    h: i32,
    xs: &mut Vec<f32, MAX_CROSSINGS>,
) where
    D: DrawTarget,
    L: Copy + Into<usize>,
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
    // without touching its edges: the whole-polygon range only bounds the union. Sized to
    // `MAX_DECODE_RINGS`; a ring past that capacity simply is not culled.
    let mut ring_y: Vec<(i32, i32), MAX_DECODE_RINGS> = Vec::new();
    {
        let mut base = 0usize;
        for &len in ring_lens {
            let len = len.into();
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
            let len = len.into();
            let ring = &screen[base..base + len];
            base += len;
            if len < 2 {
                continue;
            }
            // Rows outside the ring's y-band cannot cross it; a ring past the `ring_y` capacity
            // falls back to the full edge test.
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
                    // A row crossing the outline more than MAX_CROSSINGS times cannot be captured
                    // whole, and pairing a truncated list would break even-odd parity and paint
                    // background-coloured gaps. An unfilled 1 px seam on the densest features
                    // beats a mis-filled span, and the buffer cannot grow inside the RAM budget.
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
            // Spans round outward, per [`fill_span`]. A feature clipped across a chunk boundary
            // becomes two polygons whose shared edge is clipped independently, so their pixel
            // staircases can disagree by up to 1 px. The overlap is cheap insurance and invisible
            // for same-coloured fills.
            fill_span(target, xs[k], xs[k + 1], y, w, color);
            k += 2;
        }
    }
}

/// Emit one outward-rounded solid span for row `y` covering `left..=right` in sub-pixel x, shared
/// by [`fill_polygon`] and [`fill_convex_quad`]. Rounding outward and clamping to the panel makes
/// adjacent fills overlap by at most 1 px rather than leave a hairline crack.
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

/// Scan-convert a convex four-point quad, a thick stroke segment's swept rectangle, as one solid
/// span per row. A convex quad crosses any scanline exactly twice, so the generic filler's
/// crossing buffer, per-row sort and per-ring bookkeeping are all avoidable: keep the minimum and
/// maximum active x and emit a single rectangle.
///
/// Byte-identical to [`fill_polygon`] on the same quad, which is a hard requirement. The per-row
/// crossing therefore uses the exact same expression rather than a hoisted reciprocal slope, whose
/// different float rounding drifts spans by a pixel on some quads. The half-open edge rule and the
/// outward span rounding are likewise the same pixel contract. The saved work is the scratch
/// buffer, the sort and the ring machinery, not the division.
///
/// `round_pt` can collapse a short or shallow segment's quad to a triangle or a zero-area sliver,
/// which still cross any row at most twice. A row that somehow presents more than two crossings
/// falls through to the same even-odd sort-and-pair [`fill_polygon`] runs, so the output can never
/// drift from the general filler.
///
/// Fixed-size and allocation-free: at most four stack edge records. `#[inline(never)]` keeps these
/// locals off the already-deep stroker frame.
#[inline(never)]
pub(crate) fn fill_convex_quad<D>(target: &mut D, quad: &[Point; 4], color: D::Color, w: i32, h: i32)
where
    D: DrawTarget,
{
    /// One non-horizontal quad side in [`fill_polygon`]'s `i`/`j` roles: `xi`/`yi` are vertex `i`,
    /// and `dx`/`dy` are `xj - xi` and `yj - yi`. The half-open active test drops horizontal sides
    /// for free, because they can never satisfy it.
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

    // Build the at most four non-horizontal edges once, iterating `i` and `j` as `fill_polygon`
    // does.
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
        // Keep the min and max active x directly, with a tiny fixed record of every crossing so a
        // degenerate row with more than two can fall back to exact even-odd.
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
            // A rounded quad presenting 3 or 4 crossings on this row is non-convex, so mirror
            // `fill_polygon` exactly and the specialized filler can never diverge from it.
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
    use super::{fill_polygon, fill_polygon_edges, PackedEdge};
    use crate::{collect::ScreenPoint, MAX_CROSSINGS, MAX_SCREEN_POINTS};
    use heapless::Vec;

    #[test]
    fn packed_edge_scan_is_pixel_identical_on_pseudo_random_polygons() {
        use embedded_graphics::{mock_display::MockDisplay, pixelcolor::BinaryColor, prelude::*};

        let mut state = 0x51f1_5e1du32;
        for case in 0..1_000 {
            let len = 3 + (state as usize % 29);
            let mut points: std::vec::Vec<Point> = std::vec::Vec::with_capacity(len);
            let mut packed: std::vec::Vec<ScreenPoint> = std::vec::Vec::with_capacity(len);
            for _ in 0..len {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let x = (state as i32).rem_euclid(96) - 16;
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let y = (state as i32).rem_euclid(96) - 16;
                points.push(Point::new(x, y));
                packed.push(ScreenPoint::checked((x, y)).unwrap());
            }
            let mut expected = MockDisplay::<BinaryColor>::new();
            let mut actual = MockDisplay::<BinaryColor>::new();
            expected.set_allow_overdraw(true);
            actual.set_allow_overdraw(true);
            let mut xs: Vec<f32, MAX_CROSSINGS> = Vec::new();
            let mut edges: Vec<PackedEdge, MAX_SCREEN_POINTS> = Vec::new();
            fill_polygon(&mut expected, &points, &[len as u16], BinaryColor::On, 64, 64, &mut xs);
            fill_polygon_edges(&mut actual, &packed, &[len as u16], BinaryColor::On, (64, 64), &mut edges, &mut xs);
            assert_eq!(actual, expected, "scan conversion drift in case {case}");
        }
    }

    #[test]
    fn fill_polygon_skips_rows_that_overflow_the_crossing_buffer() {
        // A scanline crossing the outline more than MAX_CROSSINGS times must be skipped, not
        // filled from the truncated crossing list, while ordinary rows still fill correctly.
        use embedded_graphics::{pixelcolor::BinaryColor, prelude::*, primitives::Rectangle};

        // Prongs give 2·P scanline crossings in the prong band, derived from the cap rather than
        // hand-picked, so growing `MAX_CROSSINGS` re-sizes the comb instead of quietly turning the
        // saturation case into a non-saturating one.
        const P: usize = MAX_CROSSINGS / 2 + 40;
        const W: i32 = 2 * P as i32; // one column per prong + its gap
        const H: i32 = 8;
        const HBASE: i32 = 4; // prongs span y ∈ [0, HBASE); a solid base sits below
        const HBOTTOM: i32 = 6;
        /// Outline capacity: the comb pushes `4·P + 1` points (see the walk below).
        const POLY_CAP: usize = 4 * P + 8;
        // The comb only proves anything if it actually overflows the buffer.
        const { assert!(2 * P > MAX_CROSSINGS, "comb must exceed MAX_CROSSINGS to exercise saturation") };

        // Records pixels painted per row, so a skipped row is distinguishable from a filled one.
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

        // A comb: P vertical 1 px prongs on a solid base. A scanline through the prongs crosses
        // both walls of every prong; one through the base crosses only the two outer walls.
        let mut poly: Vec<Point, POLY_CAP> = Vec::new();
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
