use crate::{
    data::RoutingData,
    model::{Point, Totals},
    router::Response,
    snap, Control, Error, Request, Result, Route, Router,
};
use std::collections::BTreeSet;

impl<P: RoutingData> Router<P> {
    /// Alternative discovery is bounded. It does not enumerate all useful routes.
    pub fn routes(&mut self, request: &Request, control: &Control<'_>) -> Result<Response> {
        let primary = self.route(request, control)?;
        let mut routes = vec![primary];
        if !request.alternatives && !request.alternatives_only {
            return Ok(Response { routes });
        }
        let base = request.profile.split('/').next().unwrap_or(&request.profile);
        for variant in ["shorter", "smoother", "less-climbing"] {
            let name = format!("{base}/{variant}");
            if name == request.profile || self.package.profile(&name).is_err() {
                continue;
            }
            let mut candidate =
                match self.route(&Request { profile: name, alternatives: false, ..request.clone() }, control) {
                    Ok(route) => route,
                    Err(Error::NoPath | Error::NoSnap(_) | Error::Limit) => continue,
                    Err(error) => return Err(error),
                };
            if let Some(reason) = tradeoff(&routes[0], &candidate) {
                candidate.reason = reason;
                if self.accept(&routes, &candidate, &request.profile)? {
                    routes.push(candidate);
                }
            }
        }
        // A single via probe preserves directed context, unlike independently joined shortest paths.
        if request.points.len() == 2 && routes[0].totals.distance_m >= 5000 {
            let primary = &routes[0];
            let a = primary.geometry[0];
            let b = *primary.geometry.last().unwrap();
            let scale = ((a[1] + b[1]) * 0.5).to_radians().cos();
            let dx = (b[0] - a[0]) * scale;
            let dy = b[1] - a[1];
            let length = dx.hypot(dy);
            let offset = (primary.totals.distance_m as f64 * 0.15).clamp(1000.0, 4000.0) / 111_195.0;
            let probes: Vec<_> = if length > 0.0 {
                [0.35, 0.65]
                    .into_iter()
                    .flat_map(|fraction| {
                        let middle = along(&primary.geometry, fraction);
                        [-1.0, 1.0].map(|side| {
                            [middle[0] - side * dy / length * offset / scale, middle[1] + side * dx / length * offset]
                        })
                    })
                    .collect()
            } else {
                vec![]
            };
            for probe in probes {
                if (control.cancelled)() {
                    return Err(Error::Cancelled);
                }
                let found = self.package.snap(
                    Point {
                        lon: (probe[0] * 1e6).round() as i32,
                        lat: (probe[1] * 1e6).round() as i32,
                        elevation: 0.0,
                    },
                    &request.profile,
                    snap::Policy { radius_m: (offset * 111_195.0).min(5000.0), ..snap::Policy::default() },
                )?;
                let Some(attachment) = found.retained.first() else {
                    continue;
                };
                let point = [attachment.projected.lon as f64 * 1e-6, attachment.projected.lat as f64 * 1e-6];
                let mut candidate = match self.route(
                    &Request {
                        points: vec![request.points[0], point, request.points[1]],
                        alternatives: false,
                        ..request.clone()
                    },
                    control,
                ) {
                    Ok(route) => route,
                    Err(Error::NoPath | Error::NoSnap(_) | Error::MissingRegion(_) | Error::Limit) => continue,
                    Err(error) => return Err(error),
                };
                candidate.reason = "corridor";
                if !distinct(&routes[0], &candidate) || !self.accept(&routes, &candidate, &request.profile)? {
                    continue;
                }
                let second = candidate.legs.pop().unwrap();
                let first = &mut candidate.legs[0];
                first.to_index = second.to_index;
                first.totals = candidate.totals.clone();
                first.roads.extend(second.roads);
                candidate.attachments.remove(1);
                routes.push(candidate);
                break;
            }
        }
        if request.alternatives_only {
            routes.remove(0);
        }
        Ok(Response { routes })
    }

    fn accept(&mut self, selected: &[Route], candidate: &Route, metric: &str) -> Result<bool> {
        if selected.iter().any(|r| r.geometry == candidate.geometry) {
            return Ok(false);
        }
        let primary = &selected[0];
        if candidate.totals.distance_m > primary.totals.distance_m * 3 / 2
            || candidate.totals.seconds > primary.totals.seconds * 1.8
        {
            return Ok(false);
        }
        let mut cost = 0u64;
        let mut previous = None;
        let mut visited = BTreeSet::new();
        for slice in candidate.legs.iter().flat_map(|l| &l.roads) {
            let road = self.package.road(slice.road)?;
            // Adjacent slices on one road are shaping points. A repeated road or its reverse is a spur.
            if previous != Some(slice.road)
                && !visited.insert((road.way, road.from.min(road.to), road.from.max(road.to)))
            {
                return Ok(false);
            }
            let endpoint = self.package.endpoint(metric, slice.road)?;
            let Some(curve) = endpoint.cost.as_ref() else {
                return Ok(false);
            };
            if let Some(before) = previous.filter(|&id| id != slice.road) {
                let arrival = self.package.endpoint(metric, before)?.arrival;
                let Some(entry) = endpoint.departures.iter().find(|d| d.state == arrival) else {
                    return Ok(false);
                };
                cost = cost.checked_add(entry.penalty).ok_or(Error::Limit)?;
            }
            previous = Some(slice.road);
            cost = cost
                .checked_add(
                    curve.prefix(slice.to).map_err(Error::InvalidData)?
                        - curve.prefix(slice.from).map_err(Error::InvalidData)?,
                )
                .ok_or(Error::Limit)?;
        }
        Ok(cost as f64 <= primary.cost as f64 * 1.35)
    }
}

fn tradeoff(primary: &Route, candidate: &Route) -> Option<&'static str> {
    let (a, b) = (&primary.totals, &candidate.totals);
    let unpaved = |t: &Totals| t.surface_m[3..].iter().sum::<u64>();
    if a.distance_m.saturating_sub(b.distance_m) >= 500.max(a.distance_m / 20) {
        Some("shorter")
    } else if a.unknown_elevation_m == 0
        && b.unknown_elevation_m == 0
        && a.ascent_m.saturating_sub(b.ascent_m) >= 100.max(a.ascent_m / 10)
    {
        Some("less_climbing")
    } else if unpaved(a).saturating_sub(unpaved(b)) >= 1000.max(unpaved(a) / 5)
        && b.surface_m[0] <= a.surface_m[0] + 250
    {
        Some("smoother")
    } else {
        None
    }
}

fn distinct(primary: &Route, candidate: &Route) -> bool {
    let point =
        |p: [f64; 2]| Point { lon: (p[0] * 1e6).round() as i32, lat: (p[1] * 1e6).round() as i32, elevation: 0.0 };
    let mut far = 0;
    let mut count = 0;
    for &coordinate in candidate.geometry.iter().step_by((candidate.geometry.len() / 100).max(1)) {
        let p = point(coordinate);
        let distance = primary
            .geometry
            .windows(2)
            .map(|s| {
                let scale = coordinate[1].to_radians().cos();
                let dx = (s[1][0] - s[0][0]) * scale;
                let dy = s[1][1] - s[0][1];
                let t = (((coordinate[0] - s[0][0]) * scale * dx + (coordinate[1] - s[0][1]) * dy)
                    / (dx * dx + dy * dy).max(f64::MIN_POSITIVE))
                .clamp(0.0, 1.0);
                p.distance(point([s[0][0] + (s[1][0] - s[0][0]) * t, s[0][1] + dy * t]))
            })
            .fold(f64::INFINITY, f64::min);
        far += usize::from(distance >= 500.0);
        count += 1;
    }
    far * 4 >= count && far > 0
}

fn along(line: &[[f64; 2]], fraction: f64) -> [f64; 2] {
    let distance = |s: &[[f64; 2]]| {
        let y = s[1][1] - s[0][1];
        let x = (s[1][0] - s[0][0]) * ((s[0][1] + s[1][1]) * 0.5).to_radians().cos();
        x.hypot(y)
    };
    let total: f64 = line.windows(2).map(distance).sum();
    let mut remaining = total * fraction;
    for pair in line.windows(2) {
        let length = distance(pair);
        if remaining <= length && length > 0.0 {
            return [
                pair[0][0] + (pair[1][0] - pair[0][0]) * remaining / length,
                pair[0][1] + (pair[1][1] - pair[0][1]) * remaining / length,
            ];
        }
        remaining -= length;
    }
    *line.last().unwrap()
}
