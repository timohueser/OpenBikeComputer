use crate::{
    answer::Edges,
    data::RoutingData,
    model::{Pace, Point, Road, Totals, BIKE, NO_ELEVATION, PUSH},
    search::{Query, Seed, Workspace},
    snap::{self, Candidate, Policy},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

#[derive(Clone, Debug, Deserialize, Serialize)]
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

#[derive(Clone, Debug)]
pub struct Slice {
    pub road: u32,
    pub from: f64,
    pub to: f64,
}
#[derive(Clone, Debug)]
pub struct Leg {
    pub start_attachment: Candidate,
    pub from_index: usize,
    pub to_index: usize,
    pub totals: Totals,
    pub roads: Vec<Slice>,
}
/// Serialize a route only through `answer`, the wire shape.
#[derive(Clone, Debug)]
pub struct Route {
    pub id: String,
    pub reason: &'static str,
    pub package: String,
    pub profile: String,
    pub cost: u64,
    pub geometry: Vec<[f64; 2]>,
    pub elevation: Vec<Option<f32>>,
    /// The facts of each edge from geometry[i] to geometry[i + 1].
    pub edges: Edges,
    /// Cumulative moving seconds at each geometry vertex.
    pub elapsed: Vec<f64>,
    pub legs: Vec<Leg>,
    pub attachments: Vec<Candidate>,
    pub snap_truncated: bool,
    pub totals: Totals,
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

#[derive(Clone)]
struct Path {
    roads: Vec<Slice>,
}

#[derive(Clone)]
struct Choice {
    source: usize,
    target: usize,
    cost: u64,
    path: Path,
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
    paths: VecDeque<(String, Choice)>,
    cached_roads: usize,
    /// Snaps by point and profile.
    snaps: VecDeque<(i32, i32, String, snap::Candidates)>,
}

impl<P: RoutingData> Router<P> {
    pub fn package(&self) -> &P {
        &self.package
    }

    pub fn snap(&mut self, point: Point, profile: &str, policy: Policy) -> Result<snap::Candidates> {
        self.package.snap(point, profile, policy)
    }

    pub fn new(mut package: P, memory_budget_bytes: usize) -> Self {
        package.set_memory_budget(memory_budget_bytes);
        Self {
            package,
            workspace: Workspace::default(),
            metric: String::new(),
            paths: VecDeque::new(),
            cached_roads: 0,
            snaps: VecDeque::new(),
        }
    }

    /// Snapping attaches only to roads in a large connected part of the profile's graph, so a leg
    /// without a path means the points are truly disconnected and the client plans around it.
    pub fn route(&mut self, request: &Request, control: &Control<'_>) -> Result<Route> {
        self.route_counted(request, control, &mut 0)
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
        let mut stages = Vec::<Vec<Option<(usize, usize, Path)>>>::new();
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
                    work.witnesses = work.witnesses.saturating_add(found.path.roads.len());
                    if work.witnesses > control.max_geometry {
                        return Err(Error::Limit);
                    }
                    let j = found.target;
                    if found.cost < next[j] {
                        next[j] = found.cost;
                        paths[j] = Some((previous[found.source], found.source, found.path));
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
        let mut attachments = vec![candidates.last().unwrap()[selected].clone()];
        let mut paths = Vec::new();
        for (index, stage) in stages.into_iter().enumerate().rev() {
            let (previous, source, path) = stage.into_iter().nth(selected).flatten().ok_or(Error::NoPath)?;
            attachments.push(candidates[index][previous].clone());
            selected = previous;
            paths.push((candidates[index][source].clone(), path));
        }
        paths.reverse();
        attachments.reverse();
        let mut route = Route {
            id: String::new(),
            reason: "primary",
            package: self.package.identity().to_owned(),
            profile: request.profile.clone(),
            cost,
            geometry: Vec::new(),
            elevation: Vec::new(),
            edges: Edges::default(),
            elapsed: Vec::new(),
            legs: Vec::new(),
            attachments,
            snap_truncated: truncated,
            totals: Totals::default(),
        };
        let closures = self.package.closures()?;
        for (start_attachment, path) in paths {
            let from_index = route.geometry.len().saturating_sub(1);
            let mut totals = Totals::default();
            for slice in &path.roads {
                if (control.cancelled)() {
                    return Err(Error::Cancelled);
                }
                let road = trim(self.package.road(slice.road)?, slice.from, slice.to);
                let mode = profile.mode(&road);
                let closure = closures.closing(slice.road, mode);
                totals.add(&road, &request.pace, &profile);
                route.totals.add(&road, &request.pace, &profile);
                let mut seconds = *route.elapsed.last().unwrap_or(&0.0);
                for (index, point) in road.shape.iter().enumerate() {
                    if index > 0 {
                        seconds +=
                            request.pace.segment_seconds(road.shape[index - 1], *point, mode != BIKE, profile.mtb_pace);
                    }
                    let coordinate = [point.lon as f64 * 1e-6, point.lat as f64 * 1e-6];
                    if route.geometry.last() == Some(&coordinate) {
                        continue;
                    }
                    if route.geometry.len() >= control.max_geometry {
                        return Err(Error::Limit);
                    }
                    if !route.geometry.is_empty() {
                        route.edges.push("surfaces", road.surface);
                        route.edges.push("pushing", mode == PUSH);
                        route.edges.push("closures", &closure);
                        route.edges.push("sac_scale", road.hiking_difficulty);
                        route.edges.push("mtb_scale", road.mtb_scale());
                    }
                    route.geometry.push(coordinate);
                    route.elapsed.push(seconds);
                    route.elevation.push((point.elevation != NO_ELEVATION).then_some(point.elevation));
                }
            }
            route.legs.push(Leg {
                start_attachment,
                from_index,
                to_index: route.geometry.len().saturating_sub(1),
                totals,
                roads: path.roads,
            });
        }
        if route.geometry.is_empty() {
            let point = route.attachments[0].projected;
            route.geometry.push([point.lon as f64 * 1e-6, point.lat as f64 * 1e-6]);
            route.elapsed.push(0.0);
            route.elevation.push((point.elevation != NO_ELEVATION).then_some(point.elevation));
        }
        route.id = crate::package::digest(
            &serde_json::to_vec(&(&route.profile, &route.geometry)).map_err(|e| Error::InvalidData(e.to_string()))?,
        );
        Ok(route)
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
        let key = format!(
            "{}:{:?}:{:?}",
            self.metric,
            sources
                .iter()
                .map(|(i, c, cost)| (*i, c.position.road, c.position.fraction.to_bits(), *cost))
                .collect::<Vec<_>>(),
            targets.iter().map(|(i, c)| (*i, c.position.road, c.position.fraction.to_bits())).collect::<Vec<_>>()
        );
        if let Some(index) = self.paths.iter().position(|(id, _)| id == &key) {
            let entry = self.paths.remove(index).unwrap();
            let result = entry.1.clone();
            self.paths.push_back(entry);
            return Ok(Some(result));
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
                            path: Path {
                                roads: vec![Slice {
                                    road: to.position.road,
                                    from: from.position.fraction,
                                    to: to.position.fraction,
                                }],
                            },
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
                best = Some(Choice { source, target, cost: found.cost, path: Path { roads } });
            }
        }
        if let Some(found) = &best {
            if found.path.roads.len() <= 65_536 {
                while self.paths.len() >= 256 || self.cached_roads + found.path.roads.len() > 65_536 {
                    if let Some((_, old)) = self.paths.pop_front() {
                        self.cached_roads -= old.path.roads.len();
                    }
                }
                self.cached_roads += found.path.roads.len();
                self.paths.push_back((key, found.clone()));
            }
        }
        Ok(best)
    }
}

fn trim(mut road: Road, from: f64, to: f64) -> Road {
    let lengths: Vec<_> = road.shape.windows(2).map(|p| p[0].distance(p[1])).collect();
    let total: f64 = lengths.iter().sum();
    let mut shape = Vec::new();
    let mut before = 0.0;
    for (pair, length) in road.shape.windows(2).zip(lengths) {
        if length > 0.0 && before + length >= from * total && before <= to * total {
            for fraction in
                [((from * total - before) / length).clamp(0.0, 1.0), ((to * total - before) / length).clamp(0.0, 1.0)]
            {
                let point = Point {
                    lat: (pair[0].lat as f64 + (pair[1].lat as f64 - pair[0].lat as f64) * fraction).round() as i32,
                    lon: (pair[0].lon as f64 + (pair[1].lon as f64 - pair[0].lon as f64) * fraction).round() as i32,
                    elevation: if pair.iter().any(|p| p.elevation == NO_ELEVATION) {
                        NO_ELEVATION
                    } else {
                        (pair[0].elevation as f64 + (pair[1].elevation as f64 - pair[0].elevation as f64) * fraction)
                            .clamp(-500.0, 9000.0) as f32
                    },
                };
                if shape.last().is_none_or(|p: &Point| p.lat != point.lat || p.lon != point.lon) {
                    shape.push(point);
                }
            }
        }
        before += length;
    }
    road.shape = shape;
    road.length_m = (total * (to - from)).round() as u32;
    let mut ascent = 0.0f64;
    let mut descent = 0.0f64;
    for pair in road.shape.windows(2) {
        if pair.iter().all(|p| p.elevation != NO_ELEVATION) {
            let difference = pair[1].elevation as f64 - pair[0].elevation as f64;
            ascent += difference.max(0.0);
            descent += (-difference).max(0.0);
        }
    }
    road.ascent_m = ascent.round() as u32;
    road.descent_m = descent.round() as u32;
    road
}
