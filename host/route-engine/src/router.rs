use crate::{
    model::{Pace, Point, Road, Surface, Totals, BIKE, NO_ELEVATION},
    package::{Package, Source},
    search::{Progress, Search},
    snap::{self, Candidate, Policy},
    storage::{Cache, Seed},
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
        Self { cancelled: &|| false, max_labels: 250_000, max_queries: 8192, max_geometry: 250_000 }
    }
}

#[derive(Clone)]
struct Path {
    cost: u64,
    roads: Vec<Slice>,
}

#[derive(Default)]
struct Work {
    queries: usize,
    witnesses: usize,
}

pub struct Router<S> {
    pub(crate) package: Package<S>,
    cache: Cache,
    metric: String,
    cache_bytes: usize,
    paths: VecDeque<(String, Path)>,
    cached_roads: usize,
    snaps: VecDeque<(i32, i32, String, snap::Candidates)>,
}

impl<S: Source> Router<S> {
    pub fn package(&self) -> &Package<S> {
        &self.package
    }

    pub fn snap(&mut self, point: Point, profile: &str, policy: Policy) -> Result<snap::Candidates> {
        self.package.snap(point, profile, policy)
    }

    pub fn new(package: Package<S>, cache_bytes: usize) -> Self {
        Self {
            package,
            cache: Cache::default(),
            metric: String::new(),
            cache_bytes,
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
        let profile = self.package.metric(&request.profile)?.profile.clone();
        if self.metric != request.profile {
            self.cache = Cache::default();
            self.metric = request.profile.clone();
        }
        let bounds = self.package.manifest.bounds;
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
                return Err(Error::MissingRegion(format!("Point {index} is outside {}", self.package.manifest.region)));
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
            for (i, from) in pair[0].iter().enumerate() {
                let previous = if request.turnarounds.contains(&stage_index) {
                    pair[0]
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.projected.lat == from.projected.lat && c.projected.lon == from.projected.lon)
                        .min_by_key(|(index, _)| costs[*index])
                        .map(|(index, _)| index)
                        .unwrap_or(i)
                } else {
                    i
                };
                if costs[previous].1 == u64::MAX {
                    continue;
                }
                for (j, to) in pair[1].iter().enumerate() {
                    work.queries += 1;
                    if work.queries > control.max_queries {
                        return Err(Error::Limit);
                    }
                    if (control.cancelled)() {
                        return Err(Error::Cancelled);
                    }
                    if let Some(path) = self.leg(from, to, control)? {
                        work.witnesses = work.witnesses.saturating_add(path.roads.len());
                        if work.witnesses > control.max_geometry {
                            return Err(Error::Limit);
                        }
                        let cost = (
                            costs[previous].0.checked_add(snap_cost(to)).ok_or(Error::Limit)?,
                            costs[previous].1.checked_add(path.cost).ok_or(Error::Limit)?,
                        );
                        if cost < next[j] {
                            next[j] = cost;
                            paths[j] = Some((previous, i, path));
                        }
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
            package: self.package.identity.clone(),
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
            warnings: self.package.manifest.warnings.clone(),
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

    fn leg(&mut self, from: &Candidate, to: &Candidate, control: &Control<'_>) -> Result<Option<Path>> {
        let key = format!(
            "{}:{}:{}:{}:{}",
            self.metric,
            from.position.road,
            from.position.fraction.to_bits(),
            to.position.road,
            to.position.fraction.to_bits()
        );
        if let Some(index) = self.paths.iter().position(|(id, _)| id == &key) {
            let entry = self.paths.remove(index).unwrap();
            let result = entry.1.clone();
            self.paths.push_back(entry);
            return Ok(Some(result));
        }
        let result = self.find_leg(from, to, control)?;
        if let Some(path) = &result {
            if path.roads.len() <= 65_536 {
                while self.paths.len() >= 256 || self.cached_roads + path.roads.len() > 65_536 {
                    if let Some((_, old)) = self.paths.pop_front() {
                        self.cached_roads -= old.roads.len();
                    }
                }
                self.cached_roads += path.roads.len();
                self.paths.push_back((key, path.clone()));
            }
        }
        Ok(result)
    }

    fn find_leg(&mut self, from: &Candidate, to: &Candidate, control: &Control<'_>) -> Result<Option<Path>> {
        let a = from.position;
        let b = to.position;
        let source = self.package.endpoint(&self.metric, a.road)?;
        let target = self.package.endpoint(&self.metric, b.road)?;
        let source_cost =
            source.cost.as_ref().ok_or_else(|| Error::InvalidData("Excluded source attachment".into()))?;
        let target_cost =
            target.cost.as_ref().ok_or_else(|| Error::InvalidData("Excluded target attachment".into()))?;
        let prefix = source_cost.prefix(a.fraction).map_err(Error::InvalidData)?;
        let suffix = target_cost.prefix(b.fraction).map_err(Error::InvalidData)?;
        let mut best = if a.road == b.road && a.fraction <= b.fraction {
            Some(Path {
                cost: suffix.checked_sub(prefix).ok_or_else(|| Error::InvalidData("Invalid partial costs".into()))?,
                roads: vec![Slice { road: a.road, from: a.fraction, to: b.fraction }],
            })
        } else {
            None
        };
        let remaining =
            source_cost.total().checked_sub(prefix).ok_or_else(|| Error::InvalidData("Invalid source cost".into()))?;
        let starts = [Seed { node: source.arrival, cost: remaining, road: a.road }];
        let ends: Vec<_> =
            target.departures.iter().map(|d| Seed { node: d.state, cost: suffix + d.penalty, road: b.road }).collect();
        let mut search = Search::new(&starts, &ends, control.max_labels).without_prefetch();
        loop {
            if (control.cancelled)() {
                return Err(Error::Cancelled);
            }
            match search.poll(&self.cache, 1024) {
                Progress::Working { .. } => {}
                Progress::NeedPages { pages } => {
                    for page in pages.into_iter().take(1) {
                        let key = self.package.key(&self.package.metric(&self.metric)?.graph, page)?;
                        let bytes = self.package.bytes(&key)?;
                        self.cache.insert_page(page, &bytes, self.cache_bytes).map_err(Error::InvalidData)?;
                    }
                }
                Progress::Done { cost, roads, .. } => {
                    if best.as_ref().is_none_or(|p| cost < p.cost) {
                        let mut slices: Vec<_> =
                            roads.into_iter().map(|road| Slice { road, from: 0.0, to: 1.0 }).collect();
                        slices[0].from = a.fraction;
                        slices.push(Slice { road: b.road, from: 0.0, to: b.fraction });
                        slices.retain(|s| s.from < s.to);
                        best = Some(Path { cost, roads: slices });
                    }
                    return Ok(best);
                }
                Progress::NoPath => return Ok(best),
                Progress::Limit => return Err(Error::Limit),
                Progress::Cancelled => return Err(Error::Cancelled),
                Progress::Invalid { message } => return Err(Error::InvalidData(message)),
            }
        }
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
