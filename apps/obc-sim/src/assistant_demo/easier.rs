//! Synthetic alternatives over the loaded map; geometry and displayed costs are mock data.

use super::*;
use obc_app::assistant_demo::easier::{Route, Routes};

fn alternative(points: &[TrackPoint], min: (i32, i32), max: (i32, i32), goal: usize) -> Vec<TrackPoint> {
    let first = points[0];
    let last = points[points.len() - 1];
    (0..129)
        .map(|i| {
            if i == 0 {
                return first;
            }
            if i == 128 {
                return last;
            }
            let t = i as f64 / 128.0;
            let index = t * (points.len() - 1) as f64;
            let a = points[index as usize];
            let b = points[(index as usize + 1).min(points.len() - 1)];
            let blend = |a: i32, b: i32| a as f64 + (b - a) as f64 * index.fract();
            let (mut lon, mut lat) = (blend(a.lon, b.lon), blend(a.lat, b.lat));
            if goal == 2 {
                lon = lon * 0.35 + (first.lon as f64 + (last.lon - first.lon) as f64 * t) * 0.65;
                lat = lat * 0.35 + (first.lat as f64 + (last.lat - first.lat) as f64 * t) * 0.65;
            } else {
                let width = ((max.0 - min.0) / 10).min(18_000) as f64;
                lon += width * (t * core::f64::consts::PI).sin() * if goal == 0 { 1.0 } else { -1.0 };
            }
            TrackPoint { lon: (lon as i32).clamp(min.0, max.0), lat: (lat as i32).clamp(min.1, max.1), ele: a.ele, t }
        })
        .collect()
}

pub(super) fn install(
    store: &mut FlatRouteStore,
    original: usize,
    points: &[TrackPoint],
    min: (i32, i32),
    max: (i32, i32),
) -> Result<&'static Routes, String> {
    let path = |points: &[TrackPoint]| -> &'static [(i32, i32)] {
        Box::leak(points.iter().map(|p| (p.lon, p.lat)).collect::<Vec<_>>().into_boxed_slice())
    };
    let current = Route { route: original, path: path(points), distance_m: 42_000, climb_m: 860, rough_m: 6_000 };
    let mut alternatives = Vec::new();
    for (i, (name, distance_m, climb_m, rough_m)) in [
        ("Less climbing", 45_000, 440, 6_000),
        ("Smoother surface", 47_000, 980, 2_000),
        ("Shorter ride", 36_000, 1_110, 6_000),
    ]
    .into_iter()
    .enumerate()
    {
        let points = alternative(points, min, max, i);
        let (route, _) = super::route(store, name, &points, None)?;
        alternatives.push(Route { route, path: path(&points), distance_m, climb_m, rough_m });
    }
    Ok(Box::leak(Box::new(Routes { current, alternatives: alternatives.try_into().unwrap() })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alternatives_keep_endpoints_and_stay_inside_the_map() {
        let min = (7_000_000, 46_000_000);
        let max = (7_040_000, 46_060_000);
        let points = local_track(min, max, (7_020_000, 46_030_000));
        for goal in 0..3 {
            let route = alternative(&points, min, max, goal);
            for i in [0, 128] {
                let original = if i == 0 { points[0] } else { points[points.len() - 1] };
                assert_eq!((route[i].lon, route[i].lat), (original.lon, original.lat));
            }
            assert!(route.iter().all(|p| (min.0..=max.0).contains(&p.lon) && (min.1..=max.1).contains(&p.lat)));
            assert!(route.iter().zip(&points).any(|(a, b)| (a.lon, a.lat) != (b.lon, b.lat)));
        }
    }
}
