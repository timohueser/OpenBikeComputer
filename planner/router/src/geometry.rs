//! Lines in microdegrees: the nearest point of a segment and points along a line.
use crate::model::{Point, NO_ELEVATION};

/// Metres per microdegree of latitude.
pub const METRES_PER_UDEG: f64 = 0.111195;

/// The point of the segment `a`–`b` nearest to `p`: its parameter from `a` (0) to `b` (1), and its
/// distance in metres. The plane is scaled at the latitude of `p`.
pub fn project(p: Point, a: Point, b: Point) -> (f64, f64) {
    let scale = (p.lat as f64 * 1e-6).to_radians().cos() * METRES_PER_UDEG;
    let xy = |q: Point| [(q.lon - p.lon) as f64 * scale, (q.lat - p.lat) as f64 * METRES_PER_UDEG];
    let (a, b) = (xy(a), xy(b));
    let d = [b[0] - a[0], b[1] - a[1]];
    let t = (-(a[0] * d[0] + a[1] * d[1]) / (d[0] * d[0] + d[1] * d[1]).max(f64::MIN_POSITIVE)).clamp(0.0, 1.0);
    (t, (a[0] + d[0] * t).hypot(a[1] + d[1] * t))
}

/// The point at parameter `t` of the segment `a`–`b`, rounded to microdegrees. An end is itself,
/// with its own elevation; between the ends, the elevation is unknown where an end's is unknown.
pub fn lerp(a: Point, b: Point, t: f64) -> Point {
    if t <= 0.0 {
        return a;
    }
    if t >= 1.0 {
        return b;
    }
    let mix = |a: f64, b: f64| a + (b - a) * t;
    Point {
        lat: mix(a.lat as f64, b.lat as f64).round() as i32,
        lon: mix(a.lon as f64, b.lon as f64).round() as i32,
        elevation: if a.elevation == NO_ELEVATION || b.elevation == NO_ELEVATION {
            NO_ELEVATION
        } else {
            mix(a.elevation as f64, b.elevation as f64) as f32
        },
    }
}

/// The metres along `line` at each of its points.
pub fn cumulative(line: &[Point]) -> Vec<f64> {
    let mut along = Vec::with_capacity(line.len());
    along.push(0.0);
    for pair in line.windows(2) {
        along.push(along[along.len() - 1] + pair[0].distance(pair[1]));
    }
    along
}

/// The segment from point `k` to point `k + 1` of a line of two or more points, whose `cumulative`
/// lengths are `along`, that holds the point `at` metres along it, and the parameter of that point.
pub fn locate(along: &[f64], at: f64) -> (usize, f64) {
    let i = along.partition_point(|&a| a < at).clamp(1, along.len() - 1);
    (i - 1, ((at - along[i - 1]) / (along[i] - along[i - 1]).max(f64::MIN_POSITIVE)).clamp(0.0, 1.0))
}

/// The point `at` metres along a line, as for `locate`.
pub fn at(line: &[Point], along: &[f64], at: f64) -> Point {
    let (k, t) = locate(along, at);
    lerp(line[k], line[k + 1], t)
}

/// The part of a line from `from` to `to` metres along it, as for `at`, without repeated points.
pub fn cut(line: &[Point], along: &[f64], from: f64, to: f64) -> Vec<Point> {
    let inner = line.iter().zip(along).filter(|&(_, &a)| a > from && a < to).map(|(p, _)| *p);
    let mut part: Vec<Point> = Vec::new();
    for point in std::iter::once(at(line, along, from)).chain(inner).chain([at(line, along, to)]) {
        if part.last().is_none_or(|p| p.lat != point.lat || p.lon != point.lon) {
            part.push(point);
        }
    }
    part
}

/// Replaces each height by the mean height over `window_m` metres of the line, centred on its
/// point, with a straight slope between points. A run of known heights is smoothed on its own. Near
/// an end of a run the window shrinks to stay centred, so a steady slope keeps its heights and the
/// end points of a run keep theirs.
pub fn smooth_heights(points: &mut [Point], window_m: f64) {
    let along = cumulative(points);
    let mut start = 0;
    while start < points.len() {
        let known = points[start].elevation != NO_ELEVATION;
        let end = points[start..]
            .iter()
            .position(|p| (p.elevation != NO_ELEVATION) != known)
            .map_or(points.len(), |n| start + n);
        if known && end - start > 2 {
            smooth_run(&mut points[start..end], &along[start..end], window_m / 2.0);
        }
        start = end;
    }
}

fn smooth_run(points: &mut [Point], along: &[f64], half: f64) {
    let z: Vec<f64> = points.iter().map(|p| p.elevation as f64).collect();
    let mut area = vec![0.0; z.len()];
    for k in 1..z.len() {
        area[k] = area[k - 1] + (along[k] - along[k - 1]) * (z[k - 1] + z[k]) / 2.0;
    }
    let integral = |x: f64| {
        let (k, t) = locate(along, x);
        area[k] + (x - along[k]) * (2.0 * z[k] + (z[k + 1] - z[k]) * t) / 2.0
    };
    let (first, last) = (along[0], along[along.len() - 1]);
    for (point, &s) in points.iter_mut().zip(along) {
        let h = half.min(s - first).min(last - s);
        if h > 0.0 {
            point.elevation = ((integral(s + h) - integral(s - h)) / (2.0 * h)) as f32;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoothing_removes_height_noise_and_keeps_slopes() {
        let line = |height: &dyn Fn(f64, usize) -> f32| -> Vec<Point> {
            (0..=67)
                .map(|k| Point {
                    lat: 0,
                    lon: (15.0 * k as f64 / METRES_PER_UDEG).round() as i32,
                    elevation: height(15.0 * k as f64, k),
                })
                .collect()
        };
        let smoothed = |mut points: Vec<Point>| {
            smooth_heights(&mut points, 100.0);
            points
        };
        let climb =
            |points: &[Point]| points.windows(2).map(|p| (p[1].elevation - p[0].elevation).max(0.0)).sum::<f32>();
        let heights = |points: &[Point]| points.iter().map(|p| p.elevation).collect::<Vec<_>>();
        let close = |a: &[f32], b: &[f32]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01);

        let noise = line(&|_, k| if k % 2 == 0 { 100.5 } else { 99.5 });
        assert_eq!(climb(&noise), 33.0);
        // A 100 m mean of a 30 m wave leaves a ripple of a few centimetres at each step.
        assert!(climb(&smoothed(noise)) < 3.0);

        let ramp = line(&|m, _| 400.0 + 0.05 * m as f32);
        assert!(close(&heights(&smoothed(ramp.clone())), &heights(&ramp)));

        let short = line(&|m, _| (0.12 * (m - 400.0)).clamp(0.0, 18.0) as f32);
        let middle = smoothed(short.clone());
        let at = |m: f64| middle[(m / 15.0) as usize].elevation;
        assert!(at(495.0) - at(450.0) >= 0.10 * 45.0);
        let mut reversed = short;
        reversed.reverse();
        let mut back = heights(&smoothed(reversed));
        back.reverse();
        assert!(close(&back, &heights(&middle)));

        let gap = smoothed(line(&|_, k| match k {
            ..30 => 0.0,
            30 => NO_ELEVATION,
            _ => 100.0,
        }));
        assert!(gap[..30].iter().all(|p| p.elevation == 0.0) && gap[31..].iter().all(|p| p.elevation == 100.0));
        assert_eq!(gap[30].elevation, NO_ELEVATION);
    }

    #[test]
    fn every_vertex_keeps_its_own_height_next_to_an_unknown_one() {
        let line: Vec<Point> = [(0, 10.0), (1_000, NO_ELEVATION), (2_000, 30.0), (3_000, 40.0)]
            .map(|(lon, elevation)| Point { lat: 0, lon, elevation })
            .to_vec();
        let along = cumulative(&line);
        assert_eq!(cut(&line, &along, 0.0, along[3]), line);
        assert_eq!(cut(&line, &along, along[2], along[3]), line[2..]);
        assert_eq!(at(&line, &along, (along[1] + along[2]) / 2.0).elevation, NO_ELEVATION);
    }
}
