use super::*;
#[inline]
fn ray_toggle(a: (i32, i32), b: (i32, i32), point: (i32, i32)) -> bool {
    let (px, py) = (point.0 as f64, point.1 as f64);
    let (ax, ay, bx, by) = (a.0 as f64, a.1 as f64, b.0 as f64, b.1 as f64);
    (ay > py) != (by > py) && px < (bx - ax) * (py - ay) / (by - ay) + ax
}

/// Whether `b` lies on the closed screen-space segment `a..c`. Removing such a vertex is exactly
/// raster-lossless. Widen before subtracting, so hostile off-panel coordinates cannot overflow.
#[inline]
fn point_on_screen_segment(a: (i32, i32), b: (i32, i32), c: (i32, i32)) -> bool {
    let (ax, ay) = (i64::from(a.0), i64::from(a.1));
    let (bx, by) = (i64::from(b.0), i64::from(b.1));
    let (cx, cy) = (i64::from(c.0), i64::from(c.1));
    (bx - ax) * (cy - ay) == (by - ay) * (cx - ax) && (bx - ax) * (bx - cx) + (by - ay) * (by - cy) <= 0
}

fn ring_affects_rect(viewport: &Viewport, ring: &[(i32, i32)], rect: (i32, i32, i32, i32), probe: (i32, i32)) -> bool {
    let Some(&last) = ring.last() else { return false };
    let mut previous = viewport.to_screen(last.0, last.1);
    let mut contains_probe = false;
    for &(lon, lat) in ring {
        let current = viewport.to_screen(lon, lat);
        if point_in_rect(current, rect) || segment_intersects_rect(previous, current, rect) {
            return true;
        }
        contains_probe ^= ray_toggle(previous, current, probe);
        previous = current;
    }
    // With no boundary entering the connected rectangle, containment is uniform across it.
    contains_probe
}

/// Append a polygon after removing only corner vertices whose two old edges and replacement chord
/// all stay wholly outside the panel. With no boundary segment entering that rectangle, winding
/// parity is constant over it, so one interior probe makes the replacement fill-equivalent.
pub(super) fn append_visible_polygon(
    viewport: &Viewport,
    points: &[(i32, i32)],
    ring_lens: &[usize],
    margin_px: i32,
    out_points: &mut Vec<ScreenPoint, MAX_FRAME_POINTS>,
    out_ring_lens: &mut Vec<u16, MAX_FRAME_RINGS>,
) -> Result<(), ()> {
    let rect = (-margin_px, -margin_px, viewport.w as i32 + margin_px, viewport.h as i32 + margin_px);
    let probe = (viewport.w as i32 / 2, viewport.h as i32 / 2);
    let mut offset = 0usize;
    for (ring_index, &len) in ring_lens.iter().enumerate() {
        let ring = points.get(offset..offset + len).ok_or(())?;
        offset += len;
        if ring_index > 0 && !ring_affects_rect(viewport, ring, rect, probe) {
            continue;
        }
        if ring.len() <= 4 {
            for &(lon, lat) in ring {
                out_points.push(ScreenPoint::checked(viewport.to_screen(lon, lat))?).map_err(|_| ())?;
            }
            out_ring_lens.push(ring.len() as u16).map_err(|_| ())?;
            continue;
        }

        let start = out_points.len();
        out_points.push(ScreenPoint::checked(viewport.to_screen(ring[0].0, ring[0].1))?).map_err(|_| ())?;
        for index in 1..ring.len() {
            let a = out_points.last().ok_or(())?.tuple();
            let b = viewport.to_screen(ring[index].0, ring[index].1);
            let c = viewport.to_screen(ring[(index + 1) % ring.len()].0, ring[(index + 1) % ring.len()].1);
            let remaining = ring.len() - index - 1;
            let kept = out_points.len() - start;
            let screen_redundant = point_on_screen_segment(a, b, c);
            let outside_equivalent = !point_in_rect(b, rect)
                && !segment_intersects_rect(a, b, rect)
                && !segment_intersects_rect(b, c, rect)
                && !segment_intersects_rect(a, c, rect)
                && (ray_toggle(a, b, probe) ^ ray_toggle(b, c, probe)) == ray_toggle(a, c, probe);
            if (!screen_redundant && !outside_equivalent) || kept + remaining < 4 {
                out_points.push(ScreenPoint::checked(b)?).map_err(|_| ())?;
            }
        }
        out_ring_lens.push((out_points.len() - start) as u16).map_err(|_| ())?;
    }
    (offset == points.len()).then_some(()).ok_or(())
}

/// Append line rings after removing vertices that project exactly onto the segment between their
/// neighbours, so strokes and dash length are unchanged while sub-pixel detail costs no points.
pub(super) fn append_visible_line(
    viewport: &Viewport,
    points: &[(i32, i32)],
    ring_lens: &[usize],
    out_points: &mut Vec<ScreenPoint, MAX_FRAME_POINTS>,
    out_ring_lens: &mut Vec<u16, MAX_FRAME_RINGS>,
) -> Result<(), ()> {
    let mut offset = 0usize;
    for &len in ring_lens {
        let ring = points.get(offset..offset + len).ok_or(())?;
        offset += len;
        let start = out_points.len();
        let Some((&first, rest)) = ring.split_first() else { return Err(()) };
        out_points.push(ScreenPoint::checked(viewport.to_screen(first.0, first.1))?).map_err(|_| ())?;
        for index in 0..rest.len().saturating_sub(1) {
            let previous = out_points.last().ok_or(())?.tuple();
            let current = viewport.to_screen(rest[index].0, rest[index].1);
            let next = viewport.to_screen(rest[index + 1].0, rest[index + 1].1);
            if !point_on_screen_segment(previous, current, next) {
                out_points.push(ScreenPoint::checked(current)?).map_err(|_| ())?;
            }
        }
        if let Some(&last) = rest.last() {
            out_points.push(ScreenPoint::checked(viewport.to_screen(last.0, last.1))?).map_err(|_| ())?;
        }
        out_ring_lens.push((out_points.len() - start) as u16).map_err(|_| ())?;
    }
    (offset == points.len()).then_some(()).ok_or(())
}
