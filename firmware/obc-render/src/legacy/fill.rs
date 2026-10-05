use super::*;
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
        xs.sort_unstable_by(crate::sort::crossings);
        let mut k = 0;
        while k + 1 < xs.len() {
            fill_span(target, xs[k], xs[k + 1], y, w, color);
            k += 2;
        }
    }
}
