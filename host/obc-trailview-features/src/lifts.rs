//! The aerialways of the TrailView feature tiles: their stations and pylons. OSM maps the stations of
//! a lift at the ends of its way and a pylon at each vertex between, but often maps no pylon. The bake
//! adds a pylon in a span that is longer than its rig allows, or under whose cable a carrier comes
//! nearer the terrain than its clearance. The pylon stands where the carrier comes nearest, as a real
//! pylon stands on a knoll. A way that runs through the end of another lift splits there: that vertex
//! is a station, not a pylon.

use std::collections::HashSet;

use crate::structures::contains;
use crate::{metres, Tags, Terrain};

/// Per rig: the height of the cable over the ground at a pylon and at a station, the longest span,
/// the sag at mid-span as a share of the span, and how far a carrier hangs under the cable and the
/// clearance that it keeps over the terrain. As RIGS of builder/web/src/lib/trailview/kinds/aerialway.ts
/// and its CABLE_AT_STATION_M of models/aerialways.ts.
struct Rig {
    pylon: f64,
    station: f64,
    span: f64,
    sag: f64,
    drop: f64,
    clear: f64,
}

const CABLE_CAR: Rig = Rig { pylon: 30.0, station: 8.4, span: f64::INFINITY, sag: 0.04, drop: 5.3, clear: 10.0 };
const GONDOLA: Rig = Rig { pylon: 14.0, station: 6.0, span: 300.0, sag: 0.02, drop: 3.7, clear: 4.0 };
const CHAIR_LIFT: Rig = Rig { pylon: 10.0, station: 4.2, span: 180.0, sag: 0.02, drop: 4.0, clear: 2.5 };
const DRAG_LIFT: Rig = Rig { pylon: 7.0, station: 4.5, span: 120.0, sag: 0.01, drop: 2.9, clear: 1.0 };

/// The rig of an `aerialway` value.
fn rig(aerialway: &str) -> Option<&'static Rig> {
    match aerialway {
        "cable_car" => Some(&CABLE_CAR),
        "gondola" | "mixed_lift" => Some(&GONDOLA),
        "chair_lift" => Some(&CHAIR_LIFT),
        "drag_lift" | "t-bar" | "j-bar" | "platter" => Some(&DRAG_LIFT),
        _ => None,
    }
}

/// Samples of the terrain per span; an added pylon stands at least GAP_M from the pylons beside it,
/// and a lift has at most MOST pylons.
const SAMPLES: usize = 16;
const GAP_M: f64 = 25.0;
const MOST: usize = 200;

/// Whether this module sets the points of a feature.
pub fn solves(kind: &str, tags: &Tags) -> bool {
    kind == "aerialway" && tags.get("aerialway").is_some_and(|a| rig(a).is_some())
}

/// The vertices of a lift's way with the added pylons.
fn pylons(tags: &Tags, points: &[[f64; 2]], terrain: &dyn Terrain) -> Vec<[f64; 2]> {
    let rig = rig(&tags["aerialway"]).expect("a lift of a rig");
    let mut out = points.to_vec();
    let ends = |i: usize, n: usize| if i == 0 || i == n - 1 { rig.station } else { rig.pylon };
    let ground = terrain.heights(&out);
    let mut z: Vec<f64> = ground.iter().enumerate().map(|(i, g)| g + ends(i, out.len())).collect();
    let t: Vec<f64> = (1..SAMPLES).map(|k| k as f64 / SAMPLES as f64).collect();
    let mut i = 0;
    while i + 1 < out.len() && out.len() < MOST {
        let ([ax, ay], [bx, by]) = (out[i], out[i + 1]);
        let span = (bx - ax).hypot(by - ay) * metres((ay + by) / 2.0);
        let sag = 4.0 * rig.sag * span;
        let under: Vec<[f64; 2]> = t.iter().map(|&u| [ax + (bx - ax) * u, ay + (by - ay) * u]).collect();
        // The terrain is f32, and so is the terrain plus the carrier's drop and clearance.
        let below: Vec<f64> = terrain
            .heights(&under)
            .iter()
            .zip(&t)
            .map(|(&g, &u)| {
                let cable = z[i] + (z[i + 1] - z[i]) * u - sag * u * (1.0 - u);
                f64::from(g as f32 + rig.drop as f32 + rig.clear as f32) - cable
            })
            .collect();
        let room: Vec<bool> =
            below.iter().zip(&t).map(|(b, &u)| b.is_finite() && span * u.min(1.0 - u) >= GAP_M).collect();
        let roomy = room.contains(&true);
        let mut worst = 0;
        for k in 0..below.len() {
            let value = |k: usize| if room[k] { below[k] } else { f64::NEG_INFINITY };
            if value(k) > value(worst) {
                worst = k;
            }
        }
        let long = span > rig.span;
        let at = if long { 0.5 } else { t[worst] };
        if (long || roomy && below[worst] > 0.0) && span * at.min(1.0 - at) >= GAP_M {
            let p = [ax + (bx - ax) * at, ay + (by - ay) * at];
            out.insert(i + 1, p);
            z.insert(i + 1, terrain.height(p) + rig.pylon);
        } else {
            i += 1;
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Metrics {
    pub lifts: usize,
    pub added_pylons: usize,
    pub housed_ends: usize,
    pub shared_ends: usize,
}

/// A lift: its tags and points.
pub type Lift = (Tags, Vec<[f64; 2]>);

fn key([x, y]: [f64; 2]) -> [u64; 2] {
    [x.to_bits(), y.to_bits()]
}

/// The lifts (tags, points) as the tiles keep them: each way split at its vertices where another lift
/// ends, with the pylons that it lacks when there is a `terrain`, and the tag `stations` where an end
/// has no station of its own: the end lies in one of the `buildings` (outer rings), or a lift before
/// it in the list has its station there.
pub fn rigged(ways: &[Lift], buildings: &[Vec<[f64; 2]>], terrain: Option<&dyn Terrain>) -> (Vec<Lift>, Metrics) {
    let ends: HashSet<[u64; 2]> =
        ways.iter().flat_map(|(_, points)| [points[0], points[points.len() - 1]]).map(key).collect();
    let mut split = Vec::new();
    for (tags, points) in ways {
        let last = points.len() - 1;
        let mut cut = vec![0];
        cut.extend((1..last).filter(|&i| ends.contains(&key(points[i]))));
        cut.push(last);
        split.extend(cut.windows(2).map(|c| (tags, &points[c[0]..=c[1]])));
    }
    let housed = inside(ways.iter().flat_map(|(_, p)| [p[0], p[p.len() - 1]]), buildings);
    let mut held = HashSet::new();
    let mut metrics = Metrics { lifts: split.len(), ..Metrics::default() };
    let mut out = Vec::new();
    for (tags, points) in split {
        let mut own = [false; 2];
        for (n, p) in [points[0], points[points.len() - 1]].into_iter().enumerate() {
            let (is_housed, is_held) = (housed.contains(&key(p)), held.contains(&key(p)));
            own[n] = !is_housed && !is_held;
            metrics.housed_ends += usize::from(is_housed);
            metrics.shared_ends += usize::from(!is_housed && is_held);
            held.insert(key(p));
        }
        let points = match terrain {
            Some(terrain) => {
                let rig = pylons(tags, points, terrain);
                metrics.added_pylons += rig.len() - points.len();
                rig
            }
            None => points.to_vec(),
        };
        let stations = match own {
            [true, false] => Some("start"),
            [false, true] => Some("end"),
            [false, false] => Some("none"),
            [true, true] => None,
        };
        let mut tags = tags.clone();
        if let Some(stations) = stations {
            tags.insert("stations".into(), stations.into());
        }
        out.push((tags, points));
    }
    (out, metrics)
}

/// The points that lie inside one of the rings.
fn inside(points: impl Iterator<Item = [f64; 2]>, rings: &[Vec<[f64; 2]>]) -> HashSet<[u64; 2]> {
    let boxes: Vec<[f64; 4]> = rings
        .iter()
        .map(|r| {
            r.iter().fold([f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY], |b, &[x, y]| {
                [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)]
            })
        })
        .collect();
    points
        .filter(|&[x, y]| {
            (0..rings.len()).any(|k| {
                let b = boxes[k];
                b[0] <= x && x <= b[2] && b[1] <= y && y <= b[3] && contains(&[rings[k].as_slice()], [x, y])
            })
        })
        .map(key)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A point at Engelberg in world coordinates, and the metres of a world unit there.
    fn origin() -> (f64, f64, f64) {
        let x0 = (8.4 + 180.0) / 360.0;
        let y0 =
            0.5 - (std::f64::consts::FRAC_PI_4 + 46.8f64.to_radians() / 2.0).tan().ln() / (2.0 * std::f64::consts::PI);
        (x0, y0, metres(y0))
    }

    /// The world point `east` and `south` metres from the origin.
    fn at(east: f64, south: f64) -> [f64; 2] {
        let (x0, y0, scale) = origin();
        [x0 + east / scale, y0 + south / scale]
    }

    fn east(point: [f64; 2]) -> f64 {
        let (x0, _, scale) = origin();
        (point[0] - x0) * scale
    }

    /// The f32 terrain of a function of metres east of the origin.
    fn terrain(height: impl Fn(f64) -> f64 + Sync) -> impl Fn(f64, f64) -> f64 + Sync {
        move |x, _| f64::from(height(east([x, 0.0])) as f32)
    }

    fn lift(aerialway: &str) -> Tags {
        Tags::from([("aerialway".into(), aerialway.into())])
    }

    #[test]
    fn a_cable_over_a_valley_keeps_its_stations_only() {
        let valley = terrain(|e| 1500.0 - 400.0 * (std::f64::consts::PI * e.clamp(0.0, 1000.0) / 1000.0).sin());
        let points = [at(0.0, 0.0), at(1000.0, 0.0)];
        assert_eq!(pylons(&lift("cable_car"), &points, &valley), points);
    }

    #[test]
    fn a_knoll_under_a_chair_lift_gets_a_pylon_on_it() {
        let knoll = terrain(|e| 1000.0 + 0.3 * e + 25.0 * (-((e - 70.0) / 15.0).powi(2)).exp());
        let out = pylons(&lift("chair_lift"), &[at(0.0, 0.0), at(160.0, 0.0)], &knoll);
        assert_eq!(out.len(), 3);
        assert!((east(out[1]) - 70.0).abs() <= 12.0);
    }

    #[test]
    fn a_long_gondola_stands_on_pylons_no_farther_apart_than_its_rig_allows() {
        let slope = terrain(|e| 1000.0 + 0.4 * e);
        let out = pylons(&lift("gondola"), &[at(0.0, 0.0), at(2000.0, 0.0)], &slope);
        assert!(out.windows(2).all(|s| east(s[1]) - east(s[0]) <= GONDOLA.span + 1e-6));
        assert_eq!((out[0], out[out.len() - 1]), (at(0.0, 0.0), at(2000.0, 0.0)));
    }

    #[test]
    fn a_steep_rise_by_a_station_gets_a_pylon_that_keeps_the_cable_over_the_ground() {
        // The ground rises 12 m in the first 20 m from the station, then gently: the worst point lies
        // too near the station for a pylon.
        let rise = |e: f64| 1000.0 + if e < 20.0 { 0.6 * e } else { 12.0 + 0.1 * (e - 20.0) };
        let out = pylons(&lift("chair_lift"), &[at(0.0, 0.0), at(170.0, 0.0)], &terrain(rise));
        assert!(out.len() > 2);
        let rig = &CHAIR_LIFT;
        let stops: Vec<f64> = out.iter().map(|&p| east(p)).collect();
        let n = out.len();
        let heights: Vec<f64> = (0..n)
            .map(|i| f64::from(rise(stops[i]) as f32) + if i == 0 || i == n - 1 { rig.station } else { rig.pylon })
            .collect();
        for k in 0..=340 {
            let e = k as f64 / 2.0;
            let span = (stops.partition_point(|&s| s <= e).clamp(1, n - 1)) - 1;
            let length = stops[span + 1] - stops[span];
            let u = (e - stops[span]) / length;
            let cable =
                heights[span] + (heights[span + 1] - heights[span]) * u - 4.0 * rig.sag * length * u * (1.0 - u);
            assert!(cable - f64::from(rise(e) as f32) > 0.0, "the cable at {e} m");
        }
    }

    #[test]
    fn unknown_terrain_adds_no_pylon() {
        let points = [at(0.0, 0.0), at(100.0, 0.0)];
        assert_eq!(pylons(&lift("drag_lift"), &points, &|_: f64, _: f64| f64::NAN), points);
    }

    #[test]
    fn a_way_through_the_end_of_another_lift_splits_there_and_one_station_stands() {
        let (a, m, b, c) = (at(0.0, 0.0), at(300.0, 0.0), at(600.0, 0.0), at(300.0, 300.0));
        let ways = [(lift("gondola"), vec![a, m, b]), (lift("chair_lift"), vec![m, c])];
        let (out, metrics) = rigged(&ways, &[], None);
        assert_eq!(out.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>(), [vec![a, m], vec![m, b], vec![m, c]]);
        let stations: Vec<Option<&str>> = out.iter().map(|(t, _)| t.get("stations").map(String::as_str)).collect();
        assert_eq!(stations, [None, Some("end"), Some("end")]);
        assert_eq!(metrics.shared_ends, 2);
    }

    #[test]
    fn an_end_in_a_building_has_no_station_of_its_own() {
        let hall = vec![at(-10.0, -10.0), at(10.0, -10.0), at(10.0, 10.0), at(-10.0, 10.0)];
        let (out, _) = rigged(&[(lift("drag_lift"), vec![at(0.0, 0.0), at(100.0, 0.0)])], &[hall], None);
        assert_eq!(out[0].0["stations"], "end");
    }
}
