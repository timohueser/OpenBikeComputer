use crate::Graph;
use planner_router::model::{Point, NO_ELEVATION};

/// Samples ground heights before metric preparation. A missing sample stays unknown.
pub fn apply(graph: &mut Graph, mut height: impl FnMut(Point) -> Result<Option<f64>, String>) -> Result<(), String> {
    // Reuse decoded raster tiles without changing road indices or turn restrictions.
    let mut order: Vec<_> = (0..graph.roads.len()).collect();
    order.sort_unstable_by_key(|&i| graph.roads[i].shape.first().map(|p| planner_router::package::cell(*p)));
    for index in order {
        let road = &mut graph.roads[index];
        if road.shape.len() < 2 {
            return Err("Terrain requires a road with at least two points".into());
        }
        let mut dense = Vec::new();
        for pair in road.shape.windows(2) {
            let pieces = (pair[0].distance(pair[1]) / 20.0).ceil().max(1.0) as usize;
            for i in 0..pieces {
                let t = i as f64 / pieces as f64;
                dense.push(Point {
                    lat: (pair[0].lat as f64 + (pair[1].lat as f64 - pair[0].lat as f64) * t).round() as i32,
                    lon: (pair[0].lon as f64 + (pair[1].lon as f64 - pair[0].lon as f64) * t).round() as i32,
                    elevation: NO_ELEVATION,
                });
            }
        }
        if let Some(&last) = road.shape.last() {
            dense.push(last);
        }
        if road.structure {
            let valid = |h: f64| h.is_finite() && (-500.0..=9000.0).contains(&h);
            let start = height(dense[0])?.filter(|h| valid(*h));
            let end = height(*dense.last().unwrap())?.filter(|h| valid(*h));
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
            for point in &mut dense {
                point.elevation = height(*point)?
                    .filter(|h| h.is_finite() && (-500.0..=9000.0).contains(h))
                    .map(|h| h as f32)
                    .unwrap_or(NO_ELEVATION);
            }
            // Symmetric distance-window filtering gives the same heights in either direction.
            let raw = dense.clone();
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
        road.ascent_m = ascent.round() as u32;
        road.descent_m = descent.round() as u32;
        road.shape = dense;
    }
    graph.warnings.retain(|w| !w.starts_with("No DEM applied"));
    graph.warnings.push("Terrain estimates use samples at most 20 m apart and a 20 m filter radius. Bridge and tunnel heights interpolate their endpoints.".into());
    Ok(())
}
