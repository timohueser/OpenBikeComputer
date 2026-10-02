use crate::{
    data::RoutingData,
    model::{Pace, Point, Road, Surface, Totals, BIKE, NO_ELEVATION},
    search::{Query, Seed, Workspace},
    snap::{self, Candidate, Policy},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};

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
    /// Interior point indices where the rider explicitly permits a reversal.
    #[serde(default)]
    pub turnarounds: Vec<usize>,
}

#[derive(Debug, Serialize)]
pub struct Response {
    pub routes: Vec<Route>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Slice {
    pub road: u32,
    pub from: f64,
    pub to: f64,
}
#[derive(Clone, Debug, Serialize)]
pub struct Leg {
    pub start_attachment: Candidate,
    pub from_index: usize,
    pub to_index: usize,
    pub totals: Totals,
    pub roads: Vec<Slice>,
}
#[derive(Clone, Debug, Serialize)]
pub struct Route {
    pub id: String,
    pub reason: &'static str,
    pub package: String,
    pub profile: String,
    pub cost: u64,
    pub geometry: Vec<[f64; 2]>,
    pub elevation: Vec<Option<f32>>,
    /// Surface of each edge from geometry[i] to geometry[i + 1].
    pub surfaces: Vec<Surface>,
    /// Whether the bicycle must be pushed along each geometry edge.
    pub pushing: Vec<bool>,
    /// Cumulative moving seconds at each geometry vertex.
    pub elapsed: Vec<f64>,
    pub legs: Vec<Leg>,
    pub attachments: Vec<Candidate>,
    pub snap_truncated: bool,
    pub totals: Totals,
    pub warnings: Vec<String>,
}

pub struct Control<'a> {
    pub cancelled: &'a dyn Fn() -> bool,
    pub max_labels: usize,
    pub max_queries: usize,
    pub max_geometry: usize,
}
impl Default for Control<'_> {
    fn default() -> Self {
        Self { cancelled: &|| false, max_labels: usize::MAX, max_queries: 8192, max_geometry: 250_000 }
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

    pub fn route(&mut self, request: &Request, control: &Control<'_>) -> Result<Route> {
        let mut work = Work::default();
        match self.route_with_policy(request, control, Policy::default(), false, &mut work) {
            Err(Error::NoPath) => {
                let policy = Policy { ambiguity_m: 50.0, max_candidates: 16, ..Policy::default() };
                let mut route = self.route_with_policy(request, control, policy, true, &mut work)?;
                route.warnings.push("The nearest roads do not connect. The route uses nearby accessible roads.".into());
                Ok(route)
            }
            result => result,
        }
    }

    fn route_with_policy(
        &mut self,
        request: &Request,
        control: &Control<'_>,
        policy: Policy,
        nearest: bool,
        work: &mut Work,
    ) -> Result<Route> {
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
            let mut found = if let Some((_, _, _, found)) = self.snaps.iter().find(|(lat, lon, metric, found)| {
                *lat == point.lat
                    && *lon == point.lon
                    && metric == &request.profile
                    && found.policy.ambiguity_m == policy.ambiguity_m
                    && found.policy.max_candidates == policy.max_candidates
            }) {
                found.clone()
            } else {
                let found = self.package.snap(point, &request.profile, policy)?;
                if self.snaps.len() == 32 {
                    self.snaps.pop_front();
                }
                self.snaps.push_back((point.lat, point.lon, request.profile.clone(), found.clone()));
                found
            };
            // Recovery must not move a point that is already on a road to a different road.
            if nearest && found.nearest_distance_m.is_some_and(|d| d <= Policy::default().ambiguity_m) {
                let cutoff = found.nearest_distance_m.unwrap() + Policy::default().ambiguity_m;
                found.retained.retain(|c| c.snap_distance_m <= cutoff);
                found.truncated = found.retained.len() > Policy::default().max_candidates;
                found.retained.truncate(Policy::default().max_candidates);
            }
            truncated |= found.truncated;
            if found.retained.is_empty() {
                return Err(Error::NoSnap(index));
            }
            candidates.push(found.retained);
        }
        // A candidate is shared by both adjacent legs. The dynamic program preserves its direction.
        let snap_cost = |c: &Candidate| if nearest { (c.snap_distance_m * 1000.0).round() as u64 } else { 0 };
        let mut costs: Vec<_> = candidates[0].iter().map(|c| (snap_cost(c), 0u64)).collect();
        let mut stages = Vec::<Vec<Option<(usize, usize, Path)>>>::new();
        for (stage_index, pair) in candidates.windows(2).enumerate() {
            let mut next = vec![(u64::MAX, u64::MAX); pair[1].len()];
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
            let mut groups = BTreeMap::<u64, Vec<(usize, &Candidate, u64)>>::new();
            for (i, from) in pair[0].iter().enumerate() {
                let prior = costs[previous[i]];
                if prior.1 != u64::MAX {
                    groups.entry(prior.0).or_default().push((i, from, prior.1));
                }
            }
            let mut targets = BTreeMap::<u64, Vec<(usize, &Candidate)>>::new();
            if stage_index + 2 == candidates.len() {
                for (j, to) in pair[1].iter().enumerate() {
                    targets.entry(snap_cost(to)).or_default().push((j, to));
                }
            }
            let mut batches = Vec::new();
            for (&snap, starts) in &groups {
                if targets.is_empty() {
                    for (j, to) in pair[1].iter().enumerate() {
                        batches.push((snap.checked_add(snap_cost(to)).ok_or(Error::Limit)?, starts, vec![(j, to)]));
                    }
                } else {
                    for (&end_snap, ends) in &targets {
                        batches.push((snap.checked_add(end_snap).ok_or(Error::Limit)?, starts, ends.clone()));
                    }
                }
            }
            batches.sort_by_key(|v| v.0);
            for (snap, starts, ends) in batches {
                if !targets.is_empty() && next.iter().any(|c| c.0 < snap) {
                    break;
                }
                if targets.is_empty() && next[ends[0].0].0 < snap {
                    continue;
                }
                work.queries += 1;
                if work.queries > control.max_queries {
                    return Err(Error::Limit);
                }
                if let Some(found) = self.leg_batch(starts, &ends, control)? {
                    work.witnesses = work.witnesses.saturating_add(found.path.roads.len());
                    if work.witnesses > control.max_geometry {
                        return Err(Error::Limit);
                    }
                    let j = found.target;
                    let score = (snap, found.cost);
                    if score < next[j] {
                        next[j] = score;
                        paths[j] = Some((previous[found.source], found.source, found.path));
                    }
                }
            }
            if next.iter().all(|cost| cost.1 == u64::MAX) {
                return Err(Error::NoPath);
            }
            costs = next;
            stages.push(paths);
        }
        let (mut selected, &(_, cost)) = costs.iter().enumerate().min_by_key(|(_, cost)| *cost).ok_or(Error::NoPath)?;
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
            surfaces: Vec::new(),
            pushing: Vec::new(),
            elapsed: Vec::new(),
            legs: Vec::new(),
            attachments,
            snap_truncated: truncated,
            totals: Totals::default(),
            warnings: self.package.warnings().to_vec(),
        };
        for (start_attachment, path) in paths {
            let from_index = route.geometry.len().saturating_sub(1);
            let mut totals = Totals::default();
            for slice in &path.roads {
                if (control.cancelled)() {
                    return Err(Error::Cancelled);
                }
                let road = trim(self.package.road(slice.road)?, slice.from, slice.to);
                totals.add(&road, &request.pace, &profile);
                route.totals.add(&road, &request.pace, &profile);
                let mut seconds = *route.elapsed.last().unwrap_or(&0.0);
                for (index, point) in road.shape.iter().enumerate() {
                    if index > 0 {
                        seconds += request.pace.segment_seconds(
                            road.shape[index - 1],
                            *point,
                            profile.walking || road.access & BIKE == 0,
                            profile.name.starts_with("mtb"),
                        );
                    }
                    let coordinate = [point.lon as f64 * 1e-6, point.lat as f64 * 1e-6];
                    if route.geometry.last() == Some(&coordinate) {
                        continue;
                    }
                    if route.geometry.len() >= control.max_geometry {
                        return Err(Error::Limit);
                    }
                    if !route.geometry.is_empty() {
                        route.surfaces.push(road.surface);
                        route.pushing.push(!profile.walking && road.access & BIKE == 0);
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
                node: e.arrival,
                cost: prior.checked_add(remaining).ok_or(Error::Limit)?,
                road: from.position.road,
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
                    road: to.position.road,
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
        let (graph, weights) = self.package.base(&self.metric)?;
        let heap_bytes = self.package.memory_budget().saturating_sub(self.package.routing_bytes(&self.metric)?);
        let ceiling = best.as_ref().map_or(u64::MAX, |b| b.cost);
        let query = Query {
            starts: &starts,
            ends: &ends,
            ceiling,
            max_labels: control.max_labels,
            max_roads: control.max_geometry,
            heap_bytes,
            cancelled: control.cancelled,
        };
        let found = if self.package.has_landmarks(&self.metric) {
            // Complete small searches before loading global distance columns.
            let probe = query.max_labels.min(262_144);
            match self.workspace.run(&graph, &weights, Query { max_labels: probe, ..query }) {
                Err(Error::Limit) if probe < query.max_labels => {
                    match self.package.landmarks(&self.metric, &starts, &ends)? {
                        Some(prepared) => self.workspace.run_with_potential(
                            &graph,
                            &weights,
                            query,
                            Some(&mut |node| Ok(prepared.get(node))),
                        )?,
                        None => self.workspace.run(&graph, &weights, query)?,
                    }
                }
                result => result?,
            }
        } else {
            self.workspace.run(&graph, &weights, query)?
        };
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
