use crate::{
    closures::Closure,
    data::RoutingData,
    geometry,
    model::{Pace, Point, Profile, Surface, Totals, BIKE, NO_ELEVATION, PUSH},
    package::digest,
    search::{Query, Seed, Workspace},
    snap::{self, Candidate, Policy, Position},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    /// Longitude, latitude. The order is never changed by the engine.
    pub points: Vec<[f64; 2]>,
    pub profile: String,
    #[serde(default)]
    pub pace: Pace,
    #[serde(default)]
    pub alternatives: bool,
    /// Asks for alternatives and leaves the primary route out of the response.
    #[serde(default)]
    pub alternatives_only: bool,
    /// Interior point indices where the rider explicitly permits a reversal.
    #[serde(default)]
    pub turnarounds: Vec<usize>,
    /// Pins the first point to a leg position of an earlier answer, so a joined leg keeps its direction.
    /// A position that is not among the point's road candidates is ignored.
    #[serde(default)]
    pub start_position: Option<String>,
    /// Pins the last point in the same way.
    #[serde(default)]
    pub end_position: Option<String>,
}

#[derive(Debug)]
pub struct Response {
    pub routes: Vec<Route>,
}

/// The part of a road that a path uses, between two fractions of its length.
#[derive(Clone, Debug)]
struct Slice {
    road: u32,
    from: f64,
    to: f64,
}

/// The part of a route on one road, from `from` to `to`, fractions of the road's length. Its edges
/// end at the points after the `end` of the piece before it, up to its own `end`; each has the
/// facts of the road.
#[derive(Clone, Debug)]
pub struct Piece {
    pub road: u32,
    pub from: f64,
    pub to: f64,
    /// The index of the last point of the piece in `Route::points`.
    pub end: usize,
    pub surface: Surface,
    /// The mode of the route on the road.
    pub mode: u8,
    /// The possible closures for that mode.
    pub closures: Option<Vec<Closure>>,
    pub sac_scale: Option<u8>,
    pub mtb_scale: Option<u8>,
}

#[derive(Clone, Debug)]
pub struct Leg {
    /// The road positions where the leg starts and ends.
    pub start: Position,
    pub end: Position,
    pub from_index: usize,
    pub to_index: usize,
    pub totals: Totals,
    pub pieces: Vec<Piece>,
}

/// Serialize a route only through `answer`, the wire shape.
#[derive(Clone, Debug)]
pub struct Route {
    pub id: String,
    pub reason: &'static str,
    pub package: String,
    pub profile: String,
    pub cost: u64,
    pub points: Vec<Point>,
    /// Cumulative moving seconds at each point.
    pub elapsed: Vec<f64>,
    pub legs: Vec<Leg>,
    pub snap_truncated: bool,
    /// The middle request point of a corridor alternative: a request through the start, this point
    /// and the finish gives the same line.
    pub via: Option<[f64; 2]>,
}

impl Route {
    pub fn pieces(&self) -> impl Iterator<Item = &Piece> {
        self.legs.iter().flat_map(|leg| &leg.pieces)
    }

    pub fn totals(&self) -> Totals {
        let mut totals = Totals::default();
        for leg in &self.legs {
            totals.add(&leg.totals);
        }
        totals
    }
}

#[derive(Clone, Copy)]
pub struct Control<'a> {
    pub cancelled: &'a dyn Fn() -> bool,
    pub max_queries: usize,
    pub max_geometry: usize,
    /// Caps the search queues below what the memory budget leaves, so that the space of a search
    /// does not depend on the cost tables in the cache.
    pub max_heap_bytes: usize,
}
impl Default for Control<'_> {
    fn default() -> Self {
        Self { cancelled: &|| false, max_queries: 8192, max_geometry: 250_000, max_heap_bytes: usize::MAX }
    }
}

struct Choice {
    source: usize,
    target: usize,
    cost: u64,
    roads: Vec<Slice>,
}

#[derive(Default)]
struct Work {
    queries: usize,
    witnesses: usize,
}

pub struct Router<P> {
    pub(crate) package: P,
    workspace: Workspace,
    metric: String,
    /// The last primary route, by its request without the alternatives flags: the web asks for the
    /// alternatives of a route right after the route.
    primary: Option<(Request, Route)>,
    /// Snaps by point and profile.
    snaps: VecDeque<(i32, i32, String, snap::Candidates)>,
}

impl<P: RoutingData> Router<P> {
    pub fn package(&self) -> &P {
        &self.package
    }

    pub fn new(mut package: P, memory_budget_bytes: usize) -> Self {
        package.set_memory_budget(memory_budget_bytes);
        Self { package, workspace: Workspace::default(), metric: String::new(), primary: None, snaps: VecDeque::new() }
    }

    /// Snapping attaches only to roads in a large connected part of the profile's graph, so a leg
    /// without a path means the points are truly disconnected and the client plans around it.
    pub fn route(&mut self, request: &Request, control: &Control<'_>) -> Result<Route> {
        self.route_counted(request, control, &mut 0)
    }

    /// The primary route of `request`, from the last primary route when only the alternatives
    /// flags differ.
    pub(crate) fn primary(&mut self, request: &Request, control: &Control<'_>, queries: &mut usize) -> Result<Route> {
        let key = Request { alternatives: false, alternatives_only: false, ..request.clone() };
        let route = match self.primary.take().filter(|(cached, _)| *cached == key) {
            Some((_, route)) => route,
            None => self.route_counted(request, control, queries)?,
        };
        self.primary = Some((key, route.clone()));
        Ok(route)
    }

    /// Routes after `queries` attachment queries of the same request, and adds the queries of
    /// this route to it.
    pub(crate) fn route_counted(
        &mut self,
        request: &Request,
        control: &Control<'_>,
        queries: &mut usize,
    ) -> Result<Route> {
        let mut work = Work { queries: *queries, ..Work::default() };
        let result = self.search(request, control, &mut work);
        *queries = work.queries;
        result
    }

    fn search(&mut self, request: &Request, control: &Control<'_>, work: &mut Work) -> Result<Route> {
        if !(2..=64).contains(&request.points.len()) {
            return Err(Error::InvalidRequest("Use 2 to 64 ordered points".into()));
        }
        if request.turnarounds.iter().any(|&i| i == 0 || i >= request.points.len() - 1) {
            return Err(Error::InvalidRequest("A turnaround must name an interior point".into()));
        }
        request.pace.validate().map_err(Error::InvalidRequest)?;
        let profile = self.package.profile(&request.profile)?.clone();
        if self.metric != request.profile {
            self.workspace.clear_heaps();
            self.metric = request.profile.clone();
        }
        let bounds = self.package.bounds();
        let mut candidates = Vec::new();
        let mut truncated = false;
        for (index, &[lon, lat]) in request.points.iter().enumerate() {
            if (control.cancelled)() {
                return Err(Error::Cancelled);
            }
            if !lon.is_finite()
                || !lat.is_finite()
                || !(-180.0..=180.0).contains(&lon)
                || !(-85.0..=85.0).contains(&lat)
            {
                return Err(Error::InvalidRequest("Invalid longitude or latitude".into()));
            }
            if lon < bounds[0] || lon > bounds[2] || lat < bounds[1] || lat > bounds[3] {
                return Err(Error::MissingRegion(format!("Point {index} is outside {}", self.package.region())));
            }
            let point =
                Point { lat: (lat * 1e6).round() as i32, lon: (lon * 1e6).round() as i32, elevation: NO_ELEVATION };
            let mut found = if let Some((.., found)) = self
                .snaps
                .iter()
                .find(|(lat, lon, metric, _)| *lat == point.lat && *lon == point.lon && metric == &request.profile)
            {
                found.clone()
            } else {
                let mut found = self.package.snap(point, &request.profile, Policy::default())?;
                if found.retained.is_empty() {
                    let far = Policy { radius_m: snap::REACH_M, ..Policy::default() };
                    found = self.package.snap(point, &request.profile, far)?;
                }
                if self.snaps.len() == 32 {
                    self.snaps.pop_front();
                }
                self.snaps.push_back((point.lat, point.lon, request.profile.clone(), found.clone()));
                found
            };
            let pin = match index {
                0 => request.start_position.as_ref(),
                last if last + 1 == request.points.len() => request.end_position.as_ref(),
                _ => None,
            };
            if let Some(pinned) = pin.and_then(|pin| found.retained.iter().find(|c| c.position.id() == *pin)) {
                found.retained = vec![pinned.clone()];
            }
            truncated |= found.truncated;
            if found.retained.is_empty() {
                return Err(Error::NoSnap(index));
            }
            candidates.push(found.retained);
        }
        // A candidate is shared by both adjacent legs. The dynamic program preserves its direction.
        let mut costs = vec![0u64; candidates[0].len()];
        let mut stages = Vec::<Vec<Option<(usize, usize, Vec<Slice>)>>>::new();
        for (stage_index, pair) in candidates.windows(2).enumerate() {
            let mut next = vec![u64::MAX; pair[1].len()];
            let mut paths = vec![None; pair[1].len()];
            let previous: Vec<_> = pair[0]
                .iter()
                .enumerate()
                .map(|(i, from)| {
                    if request.turnarounds.contains(&stage_index) {
                        pair[0]
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| {
                                c.projected.lat == from.projected.lat && c.projected.lon == from.projected.lon
                            })
                            .min_by_key(|(index, _)| costs[*index])
                            .map(|(index, _)| index)
                            .unwrap_or(i)
                    } else {
                        i
                    }
                })
                .collect();
            let starts: Vec<_> = pair[0]
                .iter()
                .enumerate()
                .filter(|(i, _)| costs[previous[*i]] != u64::MAX)
                .map(|(i, from)| (i, from, costs[previous[i]]))
                .collect();
            // The last leg needs only its best end. An inner leg needs the best cost to each
            // candidate of the next point, so each is one query.
            let ends: Vec<Vec<(usize, &Candidate)>> = if stage_index + 2 == candidates.len() {
                vec![pair[1].iter().enumerate().collect()]
            } else {
                pair[1].iter().enumerate().map(|end| vec![end]).collect()
            };
            for ends in ends {
                work.queries += 1;
                if work.queries > control.max_queries {
                    return Err(Error::Limit);
                }
                if let Some(found) = self.leg_batch(&starts, &ends, control)? {
                    work.witnesses = work.witnesses.saturating_add(found.roads.len());
                    if work.witnesses > control.max_geometry {
                        return Err(Error::Limit);
                    }
                    let j = found.target;
                    if found.cost < next[j] {
                        next[j] = found.cost;
                        paths[j] = Some((previous[found.source], found.source, found.roads));
                    }
                }
            }
            if next.iter().all(|&cost| cost == u64::MAX) {
                return Err(Error::NoPath);
            }
            costs = next;
            stages.push(paths);
        }
        let (mut selected, &cost) = costs.iter().enumerate().min_by_key(|(_, cost)| *cost).ok_or(Error::NoPath)?;
        // Each leg departs from its start candidate and arrives at the candidate that the next leg
        // continues from; they differ only at a turnaround.
        let mut paths = Vec::new();
        for (index, stage) in stages.into_iter().enumerate().rev() {
            let end = candidates[index + 1][selected].position;
            let (previous, source, roads) = stage.into_iter().nth(selected).flatten().ok_or(Error::NoPath)?;
            paths.push((&candidates[index][source], end, roads));
            selected = previous;
        }
        paths.reverse();
        let (points, elapsed, legs) = self.line(request, &profile, control, &paths)?;
        let mut identity = request.profile.as_bytes().to_vec();
        identity.push(0);
        for point in &points {
            identity.extend(point.lon.to_le_bytes());
            identity.extend(point.lat.to_le_bytes());
        }
        Ok(Route {
            id: digest(&identity),
            reason: "primary",
            package: self.package.identity().to_owned(),
            profile: request.profile.clone(),
            cost,
            points,
            elapsed,
            legs,
            snap_truncated: truncated,
            via: None,
        })
    }

    /// The points, moving seconds and legs of the chosen paths. Each path is a leg from its start
    /// candidate to its end position.
    fn line(
        &self,
        request: &Request,
        profile: &Profile,
        control: &Control<'_>,
        paths: &[(&Candidate, Position, Vec<Slice>)],
    ) -> Result<(Vec<Point>, Vec<f64>, Vec<Leg>)> {
        let closures = self.package.closures()?;
        let mut points: Vec<Point> = Vec::new();
        let mut elapsed = Vec::new();
        let mut legs = Vec::with_capacity(paths.len());
        for &(start, end, ref roads) in paths {
            let from_index = points.len().saturating_sub(1);
            let mut totals = Totals::default();
            let mut pieces = Vec::with_capacity(roads.len());
            for slice in roads {
                if (control.cancelled)() {
                    return Err(Error::Cancelled);
                }
                let (shape, total, mut piece) = self.package.with_road(slice.road, |road| {
                    let along = geometry::cumulative(&road.shape);
                    let total = along[along.len() - 1];
                    let piece = Piece {
                        road: slice.road,
                        from: slice.from,
                        to: slice.to,
                        end: 0,
                        surface: road.surface,
                        mode: profile.mode(road),
                        closures: None,
                        sac_scale: road.hiking_difficulty,
                        mtb_scale: road.mtb_scale(),
                    };
                    (geometry::cut(&road.shape, &along, slice.from * total, slice.to * total), total, piece)
                })?;
                piece.closures = closures.closing(slice.road, piece.mode);
                let length = (total * (slice.to - slice.from)).round() as u64;
                totals.distance_m += length;
                totals.surface_m[piece.surface as usize] += length;
                if piece.mode == PUSH {
                    totals.pushing_m += length;
                }
                if shape.iter().any(|p| p.elevation == NO_ELEVATION) {
                    totals.unknown_elevation_m += length;
                }
                let (mut ascent, mut descent) = (0.0f64, 0.0f64);
                let mut seconds = elapsed.last().copied().unwrap_or(0.0);
                for (index, &point) in shape.iter().enumerate() {
                    if index > 0 {
                        let before = shape[index - 1];
                        let step = request.pace.segment_seconds(before, point, piece.mode != BIKE, profile.mtb_pace);
                        seconds += step;
                        totals.seconds += step;
                        if before.elevation != NO_ELEVATION && point.elevation != NO_ELEVATION {
                            let rise = point.elevation as f64 - before.elevation as f64;
                            ascent += rise.max(0.0);
                            descent += (-rise).max(0.0);
                        }
                    }
                    if points.last().is_some_and(|p| p.lat == point.lat && p.lon == point.lon) {
                        continue;
                    }
                    if points.len() >= control.max_geometry {
                        return Err(Error::Limit);
                    }
                    points.push(point);
                    elapsed.push(seconds);
                }
                totals.ascent_m += ascent.round() as u64;
                totals.descent_m += descent.round() as u64;
                piece.end = points.len() - 1;
                pieces.push(piece);
            }
            legs.push(Leg {
                start: start.position,
                end,
                from_index,
                to_index: points.len().saturating_sub(1),
                totals,
                pieces,
            });
        }
        if points.is_empty() {
            points.push(paths[0].0.projected);
            elapsed.push(0.0);
        }
        Ok((points, elapsed, legs))
    }

    fn leg_batch(
        &mut self,
        sources: &[(usize, &Candidate, u64)],
        targets: &[(usize, &Candidate)],
        control: &Control<'_>,
    ) -> Result<Option<Choice>> {
        if (control.cancelled)() {
            return Err(Error::Cancelled);
        }
        let mut starts = Vec::new();
        let mut source_prefixes = Vec::new();
        for &(index, from, prior) in sources {
            let e = self.package.endpoint(&self.metric, from.position.road)?;
            let cost = e.cost.ok_or_else(|| Error::InvalidData("Excluded source attachment".into()))?;
            let prefix = cost.prefix(from.position.fraction).map_err(Error::InvalidData)?;
            let remaining =
                cost.total().checked_sub(prefix).ok_or_else(|| Error::InvalidData("Invalid source cost".into()))?;
            source_prefixes.push(prefix);
            starts.push(Seed {
                node: from.position.road,
                cost: prior.checked_add(remaining).ok_or(Error::Limit)?,
                choice: u8::try_from(index).map_err(|_| Error::Limit)?,
            });
        }
        let mut ends = Vec::new();
        let mut end_targets = Vec::new();
        let mut best: Option<Choice> = None;
        for (target, &(index, to)) in targets.iter().enumerate() {
            let e = self.package.endpoint(&self.metric, to.position.road)?;
            let cost = e.cost.ok_or_else(|| Error::InvalidData("Excluded target attachment".into()))?;
            let prefix = cost.prefix(to.position.fraction).map_err(Error::InvalidData)?;
            for d in e.departures {
                ends.push(Seed {
                    node: d.state,
                    cost: prefix.checked_add(d.penalty).ok_or(Error::Limit)?,
                    choice: u8::try_from(index).map_err(|_| Error::Limit)?,
                });
                end_targets.push(target);
            }
            for (source, &(source_index, from, prior)) in sources.iter().enumerate() {
                if from.position.road == to.position.road && from.position.fraction <= to.position.fraction {
                    let cost = prefix
                        .checked_sub(source_prefixes[source])
                        .ok_or_else(|| Error::InvalidData("Invalid partial road costs".into()))?;
                    let total = prior.checked_add(cost).ok_or(Error::Limit)?;
                    if best.as_ref().is_none_or(|b| (total, index, source_index) < (b.cost, b.target, b.source)) {
                        best = Some(Choice {
                            source: source_index,
                            target: index,
                            cost: total,
                            roads: vec![Slice {
                                road: to.position.road,
                                from: from.position.fraction,
                                to: to.position.fraction,
                            }],
                        });
                    }
                }
            }
        }
        let prepared = self.package.prepared(&self.metric)?;
        let heap_bytes = (self.package.memory_budget().saturating_sub(self.package.routing_bytes(&self.metric)?))
            .min(control.max_heap_bytes);
        let query = Query {
            starts: &starts,
            ends: &ends,
            ceiling: best.as_ref().map_or(u64::MAX, |b| b.cost),
            max_roads: control.max_geometry,
            heap_bytes,
            cancelled: control.cancelled,
        };
        let potential = match &prepared.guide {
            Some(guide) => Some(guide.potential(&starts, &ends)?),
            None => None,
        };
        let found = self.workspace.run_with_potential(&prepared.graph, &prepared.costs, query, potential.as_ref())?;
        if let Some(found) = found {
            let &(source, from, _) = &sources[found.source];
            let &(target, to) = &targets[end_targets[found.target]];
            if best.as_ref().is_none_or(|b| (found.cost, target, source) < (b.cost, b.target, b.source)) {
                let mut roads: Vec<_> =
                    found.roads.into_iter().map(|road| Slice { road, from: 0.0, to: 1.0 }).collect();
                roads[0].from = from.position.fraction;
                roads.push(Slice { road: to.position.road, from: 0.0, to: to.position.fraction });
                roads.retain(|r| r.from < r.to);
                best = Some(Choice { source, target, cost: found.cost, roads });
            }
        }
        Ok(best)
    }
}
