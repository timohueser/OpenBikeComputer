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
            smooth(&mut dense);
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

/// Symmetric 20 m windows preserve endpoint heights and never cross unknown terrain.
fn smooth(points: &mut [Point]) {
    let heights: Vec<_> = points.iter().map(|p| p.elevation).collect();
    let lengths: Vec<_> = std::iter::once(0.0).chain(points.windows(2).map(|p| p[0].distance(p[1]))).collect();
    let (mut left, mut right) = (0, 0);
    let (mut behind, mut ahead, mut sum) = (Sum::default(), Sum::default(), Sum::default());
    for i in 0..points.len() {
        if heights[i] == NO_ELEVATION {
            (left, right) = (i + 1, i + 1);
            (behind, ahead, sum) = (Sum::default(), Sum::default(), Sum::default());
            continue;
        }
        if i > left {
            behind.add(lengths[i]);
            if right > i {
                ahead.add(-lengths[i]);
            } else {
                ahead = Sum::default();
            }
        }
        while left < i && behind.get() > 20.0 {
            sum.add(-f64::from(heights[left]));
            left += 1;
            behind.add(-lengths[left]);
        }
        if left == i {
            behind = Sum::default();
        }
        while right < points.len() && heights[right] != NO_ELEVATION {
            let length = if right == i { 0.0 } else { lengths[right] };
            if ahead.get() + length > 20.0 {
                break;
            }
            ahead.add(length);
            sum.add(f64::from(heights[right]));
            right += 1;
        }
        if i != 0 && i + 1 != points.len() {
            points[i].elevation = (sum.get() / (right - left) as f64) as f32;
        }
    }
}

/// Keep window distances local and compensated so long ways do not lose precision at 20 m.
#[derive(Default)]
struct Sum {
    value: f64,
    correction: f64,
}

impl Sum {
    fn add(&mut self, value: f64) {
        let next = self.value + value;
        self.correction +=
            if self.value.abs() >= value.abs() { (self.value - next) + value } else { (value - next) + self.value };
        self.value = next;
    }

    fn get(&self) -> f64 {
        self.value + self.correction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(points: &[Point]) -> Vec<Point> {
        let mut result = points.to_vec();
        for i in 1..points.len().saturating_sub(1) {
            if points[i].elevation == NO_ELEVATION {
                continue;
            }
            let (mut sum, mut count) = (f64::from(points[i].elevation), 1);
            for direction in [-1isize, 1] {
                let mut index = i as isize + direction;
                let mut distance = 0.0;
                while index >= 0 && (index as usize) < points.len() {
                    let current = points[index as usize];
                    distance += current.distance(points[(index - direction) as usize]);
                    if distance > 20.0 || current.elevation == NO_ELEVATION {
                        break;
                    }
                    sum += f64::from(current.elevation);
                    count += 1;
                    index += direction;
                }
            }
            result[i].elevation = (sum / f64::from(count)) as f32;
        }
        result
    }

    fn check(points: &[Point]) {
        let expected = reference(points);
        let mut actual = points.to_vec();
        smooth(&mut actual);
        let mut reverse: Vec<_> = points.iter().rev().copied().collect();
        smooth(&mut reverse);
        reverse.reverse();
        for ((actual, reverse), expected) in actual.iter().zip(&reverse).zip(&expected) {
            if expected.elevation == NO_ELEVATION {
                assert_eq!(actual.elevation, NO_ELEVATION);
                assert_eq!(reverse.elevation, NO_ELEVATION);
            } else {
                // Window sums change the addition order; the stored height has f32 precision.
                let tolerance = expected.elevation.abs().max(1.0) * f32::EPSILON;
                assert!((actual.elevation - expected.elevation).abs() <= tolerance);
                assert!((reverse.elevation - expected.elevation).abs() <= tolerance);
            }
        }
        assert_eq!(actual.first(), points.first());
        assert_eq!(actual.last(), points.last());
    }

    #[test]
    fn sliding_windows_match_distance_scans_with_dense_samples_duplicates_and_gaps() {
        for step in [0, 1, 25, 180, 250] {
            let mut points: Vec<_> = (0..513)
                .map(|i| Point {
                    lat: 47_000_000,
                    lon: 10_000_000 + i * step / 3,
                    elevation: (i * 17 % 109) as f32 / 7.0 - 5.0,
                })
                .collect();
            check(&points);
            for i in [0, 1, 5, 6, 7, 250, 511, 512] {
                points[i].elevation = NO_ELEVATION;
            }
            check(&points);
        }
    }

    #[test]
    fn long_ways_keep_samples_on_the_correct_side_of_the_window_boundary() {
        for lat in [83_525_124, 83_525_125] {
            let points: Vec<_> = (0..120_000)
                .map(|i| Point {
                    lat,
                    lon: -170_000_000 + i * 1595,
                    elevation: match i % 11 {
                        0 => 8999.75,
                        1 => -499.75,
                        _ => (i % 7) as f32 * 1e-6 - 3e-6,
                    },
                })
                .collect();
            let step = points[0].distance(points[1]);
            assert!(if lat == 83_525_124 { step > 20.0 } else { step < 20.0 });
            check(&points);
        }
    }
}
