use crate::{
    data::RoutingData,
    geometry::{self, METRES_PER_UDEG},
    model::{Point, Totals},
    router::Response,
    snap, Control, Error, Request, Result, Route, Router,
};
use std::collections::BTreeSet;

impl<P: RoutingData> Router<P> {
    /// Alternative discovery is bounded. It does not enumerate all useful routes. All routes share
    /// the query budget of `control`.
    pub fn routes(&mut self, request: &Request, control: &Control<'_>) -> Result<Response> {
        let mut queries = 0;
        let mut routes = vec![self.primary(request, control, &mut queries)?];
        if request.alternatives || request.alternatives_only {
            match self.alternatives(request, control, &mut queries, &mut routes) {
                // The deadline or a limit ends the discovery, and the answer keeps what it found.
                Ok(()) | Err(Error::Cancelled | Error::Limit) => {}
                Err(error) => return Err(error),
            }
            if request.alternatives_only {
                routes.remove(0);
            }
        }
        Ok(Response { routes })
    }

    /// Adds the accepted alternatives to `routes`, which holds the primary route.
    fn alternatives(
        &mut self,
        request: &Request,
        control: &Control<'_>,
        queries: &mut usize,
        routes: &mut Vec<Route>,
    ) -> Result<()> {
        let base = request.profile.split('/').next().unwrap_or(&request.profile);
        for variant in ["shorter", "less-climbing"] {
            let name = format!("{base}/{variant}");
            if name == request.profile || self.package.profile(&name).is_err() {
                continue;
            }
            let goal = Request { profile: name, alternatives: false, ..request.clone() };
            let mut candidate = match self.route_counted(&goal, control, queries) {
                Ok(route) => route,
                Err(Error::NoPath | Error::NoSnap(_)) => continue,
                Err(error) => return Err(error),
            };
            if let Some(reason) = tradeoff(&routes[0], &candidate) {
                candidate.reason = reason;
                if self.accept(routes, &candidate, &request.profile)? {
                    routes.push(candidate);
                }
            }
        }
        // A single via probe preserves directed context, unlike independently joined shortest paths.
        let distance_m = routes[0].totals().distance_m;
        if request.points.len() == 2 && distance_m >= 5000 {
            let line = &routes[0].points;
            let (a, b) = (line[0], line[line.len() - 1]);
            let scale = ((a.lat + b.lat) as f64 * 0.5e-6).to_radians().cos();
            let dx = (b.lon - a.lon) as f64 * scale;
            let dy = (b.lat - a.lat) as f64;
            let length = dx.hypot(dy);
            let offset_m = (distance_m as f64 * 0.15).clamp(1000.0, 4000.0);
            let offset = offset_m / METRES_PER_UDEG;
            let along = geometry::cumulative(line);
            let probes: Vec<_> = if length > 0.0 {
                [0.35, 0.65]
                    .into_iter()
                    .flat_map(|fraction| {
                        let middle = geometry::at(line, &along, fraction * along[along.len() - 1]);
                        [-1.0, 1.0].map(|side| Point {
                            lon: (middle.lon as f64 - side * dy / length * offset / scale).round() as i32,
                            lat: (middle.lat as f64 + side * dx / length * offset).round() as i32,
                            elevation: 0.0,
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
                let policy = snap::Policy { radius_m: offset_m.min(5000.0), ..snap::Policy::default() };
                let found = self.package.snap(probe, &request.profile, policy)?;
                let Some(attachment) = found.retained.first() else {
                    continue;
                };
                let via = [attachment.projected.lon as f64 / 1e6, attachment.projected.lat as f64 / 1e6];
                let probed = Request {
                    points: vec![request.points[0], via, request.points[1]],
                    alternatives: false,
                    ..request.clone()
                };
                let mut candidate = match self.route_counted(&probed, control, queries) {
                    Ok(route) => route,
                    Err(Error::NoPath | Error::NoSnap(_) | Error::MissingRegion(_)) => continue,
                    Err(error) => return Err(error),
                };
                candidate.reason = "corridor";
                if !distinct(&routes[0], &candidate)
                    || self.spur(&candidate)?
                    || !self.accept(routes, &candidate, &request.profile)?
                {
                    continue;
                }
                let second = candidate.legs.pop().unwrap();
                let first = &mut candidate.legs[0];
                first.to_index = second.to_index;
                first.end = second.end;
                first.totals.add(&second.totals);
                first.pieces.extend(second.pieces);
                candidate.via = Some(via);
                routes.push(candidate);
                break;
            }
        }
        Ok(())
    }

    /// Whether a route goes out along a road and back, as to a probe beside the road. Adjacent
    /// pieces on one road meet at a point; a repeated road or its reverse is a spur.
    fn spur(&self, route: &Route) -> Result<bool> {
        let mut previous = None;
        let mut visited = BTreeSet::new();
        for piece in route.pieces() {
            if previous != Some(piece.road) {
                let way = self.package.with_road(piece.road, |r| (r.way, r.from.min(r.to), r.from.max(r.to)))?;
                if !visited.insert(way) {
                    return Ok(true);
                }
            }
            previous = Some(piece.road);
        }
        Ok(false)
    }

    fn accept(&self, selected: &[Route], candidate: &Route, metric: &str) -> Result<bool> {
        if selected.iter().any(|r| r.points == candidate.points) {
            return Ok(false);
        }
        let primary = &selected[0];
        let (a, b) = (primary.totals(), candidate.totals());
        if b.distance_m > a.distance_m * 3 / 2 || b.seconds > a.seconds * 1.8 {
            return Ok(false);
        }
        // A route of the request's profile carries its cost; a route of another profile is costed again.
        let cost = if candidate.profile == metric {
            candidate.cost
        } else {
            match self.cost(candidate, metric)? {
                Some(cost) => cost,
                None => return Ok(false),
            }
        };
        Ok(cost as f64 <= primary.cost as f64 * 1.35)
    }

    /// The cost of a route with `metric`, or none when the metric excludes one of its roads or turns.
    fn cost(&self, route: &Route, metric: &str) -> Result<Option<u64>> {
        let mut cost = 0u64;
        for leg in &route.legs {
            // A leg continues the previous leg on its road, or turns back there at no turn cost.
            let mut previous = None;
            for piece in &leg.pieces {
                let endpoint = self.package.endpoint(metric, piece.road)?;
                let Some(curve) = endpoint.cost.as_ref() else {
                    return Ok(None);
                };
                if let Some(before) = previous.filter(|&id| id != piece.road) {
                    let Some(entry) = endpoint.departures.iter().find(|d| d.state == before) else {
                        return Ok(None);
                    };
                    cost = cost.checked_add(entry.penalty).ok_or(Error::Limit)?;
                }
                previous = Some(piece.road);
                cost = cost
                    .checked_add(
                        curve.prefix(piece.to).map_err(Error::InvalidData)?
                            - curve.prefix(piece.from).map_err(Error::InvalidData)?,
                    )
                    .ok_or(Error::Limit)?;
            }
        }
        Ok(Some(cost))
    }
}

fn tradeoff(primary: &Route, candidate: &Route) -> Option<&'static str> {
    let (a, b) = (&primary.totals(), &candidate.totals());
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

/// Whether a quarter or more of about 100 points along the candidate are 500 m or more from the
/// primary route.
fn distinct(primary: &Route, candidate: &Route) -> bool {
    let mut far = 0;
    let mut count = 0;
    for &p in candidate.points.iter().step_by((candidate.points.len() / 100).max(1)) {
        let distance =
            primary.points.windows(2).map(|s| geometry::project(p, s[0], s[1]).1).fold(f64::INFINITY, f64::min);
        far += usize::from(distance >= 500.0);
        count += 1;
    }
    far * 4 >= count && far > 0
}
