use planner_router::model::{Point, NO_ELEVATION};

/// The ground height at a point, or `None` where it is unknown.
pub type Height<'a> = &'a mut dyn FnMut(Point) -> Result<Option<f64>, String>;

/// The warning of a package with terrain.
pub const WARNING: &str = "Terrain estimates use samples at most 20 m apart and a 20 m filter radius. Bridge and tunnel heights interpolate their endpoints.";

/// Resamples one road `shape` to points at most 20 m apart and writes them with their heights into
/// `out`, which has `resampled_len(shape)` points. A missing sample stays unknown. Returns the
/// ascent and the descent in metres.
pub fn sample(shape: &[Point], structure: bool, height: Height, out: &mut [Point]) -> Result<(u32, u32), String> {
    if shape.len() < 2 {
        return Err("Terrain requires a road with at least two points".into());
    }
    let mut next = 0;
    for pair in shape.windows(2) {
        let pieces = pieces(pair[0], pair[1]);
        for i in 0..pieces {
            let t = i as f64 / pieces as f64;
            out[next] = Point {
                lat: (pair[0].lat as f64 + (pair[1].lat as f64 - pair[0].lat as f64) * t).round() as i32,
                lon: (pair[0].lon as f64 + (pair[1].lon as f64 - pair[0].lon as f64) * t).round() as i32,
                elevation: NO_ELEVATION,
            };
            next += 1;
        }
    }
    out[next] = shape[shape.len() - 1];
    let dense = &mut out[..=next];
    if structure {
        let valid = |h: f64| h.is_finite() && (-500.0..=9000.0).contains(&h);
        let start = height(dense[0])?.filter(|h| valid(*h));
        let end = height(dense[next])?.filter(|h| valid(*h));
        let total: f64 = dense.windows(2).map(|p| p[0].distance(p[1])).sum();
        let mut distance = 0.0;
        for i in 0..dense.len() {
            if i > 0 {
                distance += dense[i - 1].distance(dense[i]);
            }
            dense[i].elevation = match (start, end) {
                (Some(a), Some(b)) => (a + (b - a) * distance / total.max(0.01)) as f32,
                _ => NO_ELEVATION,
            };
        }
    } else {
        for point in dense.iter_mut() {
            point.elevation = height(*point)?
                .filter(|h| h.is_finite() && (-500.0..=9000.0).contains(h))
                .map(|h| h as f32)
                .unwrap_or(NO_ELEVATION);
        }
        // Symmetric distance-window filtering gives the same heights in either direction.
        let raw = dense.to_vec();
        for (i, point) in dense.iter_mut().enumerate() {
            if i == 0 || i + 1 == raw.len() || raw[i].elevation == NO_ELEVATION {
                continue;
            }
            let mut sum = raw[i].elevation as f64;
            let mut count = 1;
            for direction in [-1isize, 1] {
                let mut index = i as isize + direction;
                let mut distance = 0.0;
                while index >= 0 && (index as usize) < raw.len() {
                    let current = raw[index as usize];
                    distance += current.distance(raw[(index - direction) as usize]);
                    if distance > 20.0 || current.elevation == NO_ELEVATION {
                        break;
                    }
                    sum += current.elevation as f64;
                    count += 1;
                    index += direction;
                }
            }
            point.elevation = (sum / count as f64) as f32;
        }
    }
    let mut ascent = 0.0f64;
    let mut descent = 0.0f64;
    for pair in dense.windows(2) {
        if pair.iter().all(|p| p.elevation != NO_ELEVATION) {
            let delta = pair[1].elevation as f64 - pair[0].elevation as f64;
            ascent += delta.max(0.0);
            descent += (-delta).max(0.0);
        }
    }
    Ok((ascent.round() as u32, descent.round() as u32))
}

/// The number of points that `sample` writes for `shape`.
pub fn resampled_len(shape: &[Point]) -> usize {
    shape.windows(2).map(|pair| pieces(pair[0], pair[1])).sum::<usize>() + usize::from(!shape.is_empty())
}

/// The pieces of 20 m at most between two shape points.
pub fn pieces(a: Point, b: Point) -> usize {
    (a.distance(b) / 20.0).ceil().max(1.0) as usize
}
