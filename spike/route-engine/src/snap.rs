//! Coordinate attachments to a frozen profile. Exactness is conditional on the retained candidates.
use crate::model::{Graph, Point, Profile, Road, BIKE, NO_ELEVATION};
use serde::Serialize;
use std::collections::{BTreeSet, HashMap, HashSet};

const CELL: i32 = 10_000;

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Position {
    pub road: u32,
    /// Fraction of the directed polyline's geometric length, from zero to one.
    pub fraction: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    pub position: Position,
    pub projected: Point,
    pub snap_distance_m: f64,
    pub segment: usize,
    pub segment_fraction: f64,
}

#[derive(Clone, Copy, Debug, Serialize)]
pub struct Policy {
    pub radius_m: f64,
    pub ambiguity_m: f64,
    pub max_candidates: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidates {
    pub policy: Policy,
    pub nearest_distance_m: Option<f64>,
    pub eligible_roads_in_radius: usize,
    pub candidates_in_ambiguity_band: usize,
    pub truncated: bool,
    pub retained: Vec<Candidate>,
}

fn cells(point: Point, radius: f64) -> Result<BTreeSet<(i32, i32)>, String> {
    if !radius.is_finite()
        || !(0.0..=5_000.0).contains(&radius)
        || point.lat.unsigned_abs() > 85_000_000
        || point.lon.unsigned_abs() > 180_000_000
    {
        return Err("Snap windows require a radius up to 5 km and latitude within ±85 degrees".into());
    }
    let lat = (radius / 0.110).ceil() as i32 + 2;
    let lon = (radius / (0.110 * (point.lat as f64 * 1e-6).to_radians().cos())).ceil() as i32 + 2;
    let mut result = BTreeSet::new();
    for y in (point.lat - lat).div_euclid(CELL)..=(point.lat + lat).div_euclid(CELL) {
        for x in (point.lon - lon).div_euclid(CELL)..=(point.lon + lon).div_euclid(CELL) {
            result.insert((y, x));
        }
    }
    Ok(result)
}

/// Index only the declared query windows; source roads are scanned once, without a routing graph copy.
pub struct SpatialIndex {
    rows: HashMap<(i32, i32), Vec<u32>>,
}

impl SpatialIndex {
    pub fn around(graph: &Graph, points: &[Point], radius_m: f64) -> Result<Self, String> {
        let mut wanted = BTreeSet::new();
        for &point in points {
            wanted.extend(cells(point, radius_m)?);
        }
        if wanted.is_empty() {
            return Err("Declare at least one snap window".into());
        }
        let min_y = wanted.iter().map(|p| p.0).min().unwrap();
        let max_y = wanted.iter().map(|p| p.0).max().unwrap();
        let min_x = wanted.iter().map(|p| p.1).min().unwrap();
        let max_x = wanted.iter().map(|p| p.1).max().unwrap();
        let mut rows: HashMap<_, Vec<_>> = wanted.iter().map(|p| (*p, Vec::new())).collect();
        for (id, road) in graph.roads.iter().enumerate() {
            if road.shape.len() < 2 {
                continue;
            }
            let south = road.shape.iter().map(|p| p.lat.div_euclid(CELL)).min().unwrap().max(min_y);
            let north = road.shape.iter().map(|p| p.lat.div_euclid(CELL)).max().unwrap().min(max_y);
            let west = road.shape.iter().map(|p| p.lon.div_euclid(CELL)).min().unwrap().max(min_x);
            let east = road.shape.iter().map(|p| p.lon.div_euclid(CELL)).max().unwrap().min(max_x);
            if south > north || west > east {
                continue;
            }
            let id = u32::try_from(id).map_err(|_| "Too many source roads")?;
            if (north as i64 - south as i64 + 1) * (east as i64 - west as i64 + 1) > 4096 {
                for (&(y, x), row) in &mut rows {
                    if (south..=north).contains(&y) && (west..=east).contains(&x) {
                        row.push(id);
                    }
                }
            } else {
                for y in south..=north {
                    for x in west..=east {
                        if let Some(row) = rows.get_mut(&(y, x)) {
                            row.push(id);
                        }
                    }
                }
            }
        }
        Ok(Self { rows })
    }

    pub fn candidates(
        &self,
        graph: &Graph,
        profile: &Profile,
        point: Point,
        policy: Policy,
    ) -> Result<Candidates, String> {
        profile.validate()?;
        if !policy.ambiguity_m.is_finite() || policy.ambiguity_m < 0.0 || policy.max_candidates == 0 {
            return Err("Invalid snap ambiguity or candidate count".into());
        }
        let mut roads = BTreeSet::new();
        for cell in cells(point, policy.radius_m)? {
            roads.extend(self.rows.get(&cell).ok_or("Query leaves the declared snap index windows")?);
        }
        let mut found = Vec::new();
        for id in roads {
            let road = graph.roads.get(id as usize).ok_or("Index refers to an absent road")?;
            if profile.cost(road).is_none() {
                continue;
            }
            if let Some(candidate) = project(id, road, point) {
                if candidate.snap_distance_m <= policy.radius_m {
                    found.push(candidate);
                }
            }
        }
        found.sort_by(|a, b| {
            a.snap_distance_m.total_cmp(&b.snap_distance_m).then(a.position.road.cmp(&b.position.road))
        });
        let eligible_roads_in_radius = found.len();
        let nearest_distance_m = found.first().map(|c| c.snap_distance_m);
        if let Some(distance) = nearest_distance_m {
            found.retain(|c| c.snap_distance_m <= distance + policy.ambiguity_m);
        }
        let candidates_in_ambiguity_band = found.len();
        let truncated = found.len() > policy.max_candidates;
        found.truncate(policy.max_candidates);
        Ok(Candidates {
            policy,
            nearest_distance_m,
            eligible_roads_in_radius,
            candidates_in_ambiguity_band,
            truncated,
            retained: found,
        })
    }
}

fn project(id: u32, road: &Road, point: Point) -> Option<Candidate> {
    let lengths: Vec<_> = road.shape.windows(2).map(|p| p[0].distance(p[1])).collect();
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 {
        return None;
    }
    let scale = (point.lat as f64 * 1e-6).to_radians().cos();
    let mut best: Option<Candidate> = None;
    let mut before = 0.0;
    for (segment, pair) in road.shape.windows(2).enumerate() {
        let ax = (pair[0].lon as f64 - point.lon as f64) * scale;
        let ay = pair[0].lat as f64 - point.lat as f64;
        let dx = (pair[1].lon as f64 - pair[0].lon as f64) * scale;
        let dy = pair[1].lat as f64 - pair[0].lat as f64;
        let denominator = dx * dx + dy * dy;
        let t = if denominator > 0.0 { (-(ax * dx + ay * dy) / denominator).clamp(0.0, 1.0) } else { 0.0 };
        let projected = Point {
            lat: (pair[0].lat as f64 + (pair[1].lat as f64 - pair[0].lat as f64) * t).round() as i32,
            lon: (pair[0].lon as f64 + (pair[1].lon as f64 - pair[0].lon as f64) * t).round() as i32,
            elevation: if pair.iter().any(|p| p.elevation == NO_ELEVATION) {
                NO_ELEVATION
            } else {
                (pair[0].elevation as f64 + (pair[1].elevation as f64 - pair[0].elevation as f64) * t).round() as i16
            },
        };
        let candidate = Candidate {
            position: Position { road: id, fraction: ((before + lengths[segment] * t) / total).clamp(0.0, 1.0) },
            projected,
            snap_distance_m: point.distance(projected),
            segment,
            segment_fraction: t,
        };
        if best.as_ref().is_none_or(|b| candidate.snap_distance_m < b.snap_distance_m) {
            best = Some(candidate);
        }
        before += lengths[segment];
    }
    best
}

/// Cumulative measure preserves the full prepared integer cost. Rounded prefix differences telescope.
pub fn prefix_cost(graph: &Graph, profile: &Profile, position: Position) -> Result<u64, String> {
    profile.validate()?;
    if !position.fraction.is_finite() || !(0.0..=1.0).contains(&position.fraction) {
        return Err("Invalid road offset".into());
    }
    let road = graph.roads.get(position.road as usize).ok_or("Unknown attachment road")?;
    let full = profile.cost(road).ok_or("Attachment road is excluded by the profile")?;
    if position.fraction == 0.0 {
        return Ok(0);
    }
    if position.fraction == 1.0 {
        return Ok(full);
    }
    let total: f64 = road.shape.windows(2).map(|p| p[0].distance(p[1])).sum();
    if total <= 0.0 {
        return Err("Cannot split a road without geometric length".into());
    }
    let mut distance = 0.0;
    let mut ascent = 0.0;
    let mut partial_ascent = 0.0;
    for pair in road.shape.windows(2) {
        let length = pair[0].distance(pair[1]);
        let up = if pair.iter().any(|p| p.elevation == NO_ELEVATION) {
            0.0
        } else {
            (pair[1].elevation as f64 - pair[0].elevation as f64).max(0.0)
        };
        let used = if length > 0.0 { ((total * position.fraction - distance) / length).clamp(0.0, 1.0) } else { 0.0 };
        partial_ascent += up * used;
        ascent += up;
        distance += length;
    }
    if ascent == 0.0 && road.ascent_m != 0 && profile.climb_weight != 0.0 {
        return Err("Cannot localize the stored climb cost without elevation samples".into());
    }
    let distance_cost = road.length_m as f64
        * profile.surface_weights[road.surface as usize]
        * profile.road_weights[road.class as usize]
        * if !profile.walking && road.access & BIKE == 0 { 4.0 } else { 1.0 };
    let climb_cost = road.ascent_m as f64 * profile.climb_weight;
    let partial =
        distance_cost * position.fraction + if ascent > 0.0 { climb_cost * partial_ascent / ascent } else { 0.0 };
    Ok((full as f64 * partial / (distance_cost + climb_cost)).round().clamp(0.0, full as f64) as u64)
}

#[derive(Clone, Debug, Serialize)]
pub struct Slice {
    pub road: u32,
    pub from: f64,
    pub to: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Route {
    pub cost: u64,
    pub slices: Vec<Slice>,
    /// One retained candidate per coordinate; adjacent legs use the same directed attachment at a via.
    pub attachments: Vec<Candidate>,
    pub core_queries: usize,
}

#[derive(Clone, Debug)]
pub struct CorePath {
    pub cost: u64,
    /// Includes the source incoming-road state. Its traversal is not charged here.
    pub roads: Vec<u32>,
}

struct Context<'a> {
    graph: &'a Graph,
    profile: &'a Profile,
    incoming: HashMap<u32, Vec<u32>>,
    outgoing: HashMap<u32, Vec<u32>>,
}

impl<'a> Context<'a> {
    fn new(graph: &'a Graph, profile: &'a Profile, points: &[Candidates]) -> Result<Self, String> {
        let mut needed = HashSet::new();
        for candidate in points.iter().flat_map(|s| &s.retained) {
            prefix_cost(graph, profile, candidate.position)?;
            let road = &graph.roads[candidate.position.road as usize];
            needed.extend([road.from, road.to]);
        }
        let mut incoming = HashMap::<_, Vec<_>>::new();
        let mut outgoing = HashMap::<_, Vec<_>>::new();
        for (id, road) in graph.roads.iter().enumerate() {
            if (!needed.contains(&road.from) && !needed.contains(&road.to)) || profile.cost(road).is_none() {
                continue;
            }
            if needed.contains(&road.from) {
                outgoing.entry(road.from).or_default().push(id as u32);
            }
            if needed.contains(&road.to) {
                incoming.entry(road.to).or_default().push(id as u32);
            }
        }
        Ok(Self { graph, profile, incoming, outgoing })
    }

    fn vertex(&self, p: Position) -> Option<u32> {
        let road = &self.graph.roads[p.road as usize];
        if p.fraction == 0.0 {
            Some(road.from)
        } else if p.fraction == 1.0 {
            Some(road.to)
        } else {
            None
        }
    }

    fn slice(&self, road: u32, from: f64, to: f64) -> Result<(u64, Slice), String> {
        let cost = prefix_cost(self.graph, self.profile, Position { road, fraction: to })?
            .checked_sub(prefix_cost(self.graph, self.profile, Position { road, fraction: from })?)
            .ok_or("Reversed directed slice")?;
        Ok((cost, Slice { road, from, to }))
    }
}

fn append(target: &mut Vec<Slice>, slice: Slice) {
    if slice.from == slice.to {
        return;
    }
    if let Some(last) = target.last_mut() {
        if last.road == slice.road && last.to == slice.from {
            last.to = slice.to;
            return;
        }
    }
    target.push(slice);
}

fn core_checked(graph: &Graph, profile: &Profile, start: u32, end: u32, path: &CorePath) -> Result<(), String> {
    if path.roads.first() != Some(&start) || path.roads.last() != Some(&end) {
        return Err("Core path endpoints differ from requested incoming-road states".into());
    }
    let mut cost = 0u64;
    for pair in path.roads.windows(2) {
        let a = graph.roads.get(pair[0] as usize).ok_or("Unknown core road")?;
        let b = graph.roads.get(pair[1] as usize).ok_or("Unknown core road")?;
        if a.to != b.from || !graph.permits_turn(pair[0], pair[1], profile.walking) {
            return Err("Illegal core transition".into());
        }
        cost = cost.checked_add(profile.cost(b).ok_or("Excluded core road")?).ok_or("Cost overflow")?;
    }
    if cost != path.cost {
        return Err("Core path cost does not match the fixed profile".into());
    }
    Ok(())
}

fn leg<F>(
    context: &Context<'_>,
    start: Position,
    end: Position,
    free_start: bool,
    free_end: bool,
    cache: &mut HashMap<(u32, u32), Option<CorePath>>,
    query: &mut F,
) -> Result<Option<(u64, Vec<Slice>)>, String>
where
    F: FnMut(u32, u32) -> Result<Option<CorePath>, String>,
{
    let mut best: Option<(u64, Vec<Slice>)> = None;
    if start.road == end.road && start.fraction <= end.fraction {
        let (cost, slice) = context.slice(start.road, start.fraction, end.fraction)?;
        let mut slices = Vec::new();
        append(&mut slices, slice);
        best = Some((cost, slices));
    }
    let start_vertex = context.vertex(start);
    let end_vertex = context.vertex(end);
    if start_vertex.is_some() && start_vertex == end_vertex && free_start && free_end {
        return Ok(Some((0, vec![])));
    }
    if free_start && start_vertex == Some(context.graph.roads[end.road as usize].from) {
        let (cost, slice) = context.slice(end.road, 0.0, end.fraction)?;
        if best.as_ref().is_none_or(|b| cost < b.0) {
            let mut slices = Vec::new();
            append(&mut slices, slice);
            best = Some((cost, slices));
        }
    }
    let mut starts = Vec::new();
    if let Some(vertex) = start_vertex.filter(|_| free_start) {
        for &road in context.outgoing.get(&vertex).into_iter().flatten() {
            let (cost, slice) = context.slice(road, 0.0, 1.0)?;
            starts.push((road, cost, slice));
        }
    } else {
        let (cost, slice) = context.slice(start.road, start.fraction, 1.0)?;
        starts.push((start.road, cost, slice));
    }
    let mut ends = Vec::new();
    if let Some(vertex) = end_vertex.filter(|_| free_end) {
        for &road in context.incoming.get(&vertex).into_iter().flatten() {
            ends.push((road, 0, None));
        }
    } else if end.fraction == 1.0 {
        ends.push((end.road, 0, None));
    } else {
        let road = &context.graph.roads[end.road as usize];
        let (cost, slice) = context.slice(end.road, 0.0, end.fraction)?;
        for &previous in context.incoming.get(&road.from).into_iter().flatten() {
            if context.graph.permits_turn(previous, end.road, context.profile.walking) {
                ends.push((previous, cost, Some(slice.clone())));
            }
        }
    }
    for (source, prefix, slice) in starts {
        for (target, suffix, tail) in &ends {
            if best.as_ref().is_some_and(|b| prefix.checked_add(*suffix).is_some_and(|c| c >= b.0)) {
                continue;
            }
            let endpoints = (source, *target);
            if let std::collections::hash_map::Entry::Vacant(entry) = cache.entry(endpoints) {
                let path = if source == *target {
                    Some(CorePath { cost: 0, roads: vec![source] })
                } else {
                    query(source, *target)?
                };
                if let Some(path) = &path {
                    core_checked(context.graph, context.profile, source, *target, path)?;
                }
                entry.insert(path);
            }
            let Some(path) = &cache[&endpoints] else {
                continue;
            };
            let cost =
                prefix.checked_add(path.cost).and_then(|c| c.checked_add(*suffix)).ok_or("Attachment cost overflow")?;
            if best.as_ref().is_some_and(|b| b.0 <= cost) {
                continue;
            }
            let mut slices = Vec::new();
            append(&mut slices, slice.clone());
            for &road in path.roads.iter().skip(1) {
                append(&mut slices, Slice { road, from: 0.0, to: 1.0 });
            }
            if let Some(tail) = tail {
                append(&mut slices, tail.clone());
            }
            best = Some((cost, slices));
        }
    }
    Ok(best)
}

/// The callback answers exact incoming-road state queries for this profile. No waypoint resets heading.
/// Outer endpoints at graph junctions are free to start or finish on any eligible incident road.
pub fn route_via<F>(
    graph: &Graph,
    profile: &Profile,
    points: &[Candidates],
    mut query: F,
) -> Result<Option<Route>, String>
where
    F: FnMut(u32, u32) -> Result<Option<CorePath>, String>,
{
    profile.validate()?;
    if points.len() < 2 {
        return Err("At least two coordinate candidate sets are required".into());
    }
    if points.iter().any(|p| p.retained.is_empty()) {
        return Ok(None);
    }
    let context = Context::new(graph, profile, points)?;
    let mut cache = HashMap::new();
    let mut states: Vec<Option<Route>> = points[0]
        .retained
        .iter()
        .map(|c| Some(Route { cost: 0, slices: vec![], attachments: vec![c.clone()], core_queries: 0 }))
        .collect();
    for next in 1..points.len() {
        let mut following = vec![None::<Route>; points[next].retained.len()];
        for (from, route) in states.iter().enumerate().filter_map(|(i, r)| r.as_ref().map(|r| (i, r))) {
            for (to, candidate) in points[next].retained.iter().enumerate() {
                let Some((cost, slices)) = leg(
                    &context,
                    points[next - 1].retained[from].position,
                    candidate.position,
                    next == 1,
                    next == points.len() - 1,
                    &mut cache,
                    &mut query,
                )?
                else {
                    continue;
                };
                let cost = route.cost.checked_add(cost).ok_or("Via route cost overflow")?;
                if following[to].as_ref().is_some_and(|old| old.cost <= cost) {
                    continue;
                }
                let mut result = route.clone();
                result.cost = cost;
                for slice in slices {
                    append(&mut result.slices, slice);
                }
                result.attachments.push(candidate.clone());
                following[to] = Some(result);
            }
        }
        states = following;
    }
    let mut best = states.into_iter().flatten().min_by_key(|r| r.cost);
    if let Some(route) = &mut best {
        route.core_queries = cache.keys().filter(|(a, b)| a != b).count();
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Surface, FOOT};
    use std::cmp::Reverse;
    use std::collections::BinaryHeap;

    fn profile() -> Profile {
        let mut p = Profile::presets().remove(0);
        p.surface_weights = [1.0; 6];
        p.road_weights = [1.0; 7];
        p.climb_weight = 0.0;
        p.pushing = false;
        p
    }

    fn graph() -> Graph {
        let points: Vec<_> = [(0, 0), (0, 1000), (1000, 1000), (1000, 0), (-500, 500), (500, 500)]
            .into_iter()
            .map(|(lat, lon)| Point { lat, lon, elevation: 0 })
            .collect();
        let roads = [(0, 1), (1, 0), (1, 2), (2, 1), (0, 3), (3, 2), (2, 3), (3, 0), (4, 5)]
            .into_iter()
            .enumerate()
            .map(|(way, (from, to))| Road {
                from,
                to,
                way: way as i64,
                length_m: 100,
                ascent_m: 0,
                descent_m: 0,
                surface: Surface::Paved,
                class: 1,
                access: BIKE | FOOT,
                difficulty: 0,
                hiking_difficulty: Some(1),
                uncertain_access: false,
                shape: vec![points[from as usize], points[to as usize]],
            })
            .collect();
        Graph { points, roads, forbidden: vec![(0, 2)], forbidden_foot: vec![], warnings: vec![] }
    }

    fn one(position: Position) -> Candidates {
        Candidates {
            policy: Policy { radius_m: 20.0, ambiguity_m: 0.0, max_candidates: 1 },
            nearest_distance_m: Some(0.0),
            eligible_roads_in_radius: 1,
            candidates_in_ambiguity_band: 1,
            truncated: false,
            retained: vec![Candidate {
                position,
                projected: Point::default(),
                snap_distance_m: 0.0,
                segment: 0,
                segment_fraction: position.fraction,
            }],
        }
    }

    fn core(graph: &Graph, profile: &Profile, start: u32, end: u32) -> Result<Option<CorePath>, String> {
        let mut distances = vec![u64::MAX; graph.roads.len()];
        let mut parents = vec![None; graph.roads.len()];
        let mut heap = BinaryHeap::from([Reverse((0, start))]);
        distances[start as usize] = 0;
        while let Some(Reverse((cost, road))) = heap.pop() {
            if distances[road as usize] != cost {
                continue;
            }
            if road == end {
                let mut roads = vec![end];
                while let Some(p) = parents[*roads.last().unwrap() as usize] {
                    roads.push(p);
                }
                roads.reverse();
                return Ok(Some(CorePath { cost, roads }));
            }
            for (next, r) in graph.roads.iter().enumerate() {
                if graph.roads[road as usize].to != r.from || !graph.permits_turn(road, next as u32, profile.walking) {
                    continue;
                }
                let Some(weight) = profile.cost(r) else {
                    continue;
                };
                let value = cost + weight;
                if value < distances[next] {
                    distances[next] = value;
                    parents[next] = Some(road);
                    heap.push(Reverse((value, next as u32)));
                }
            }
        }
        Ok(None)
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Vertex {
        Junction(u32),
        Interior(u32, u64),
    }

    fn vertex(graph: &Graph, p: Position) -> Vertex {
        let road = &graph.roads[p.road as usize];
        if p.fraction == 0.0 {
            Vertex::Junction(road.from)
        } else if p.fraction == 1.0 {
            Vertex::Junction(road.to)
        } else {
            Vertex::Interior(p.road, p.fraction.to_bits())
        }
    }

    /// Independent explicit edge subdivision, with uniform per-road costs in this fixture.
    fn split_oracle(graph: &Graph, profile: &Profile, start: Position, end: Position) -> Option<u64> {
        let source = vertex(graph, start);
        let target = vertex(graph, end);
        if source == target {
            return Some(0);
        }
        let mut edges = Vec::new();
        for (id, road) in graph.roads.iter().enumerate() {
            let Some(cost) = profile.cost(road) else {
                continue;
            };
            let mut cuts = vec![0.0, 1.0];
            for p in [start, end] {
                if p.road == id as u32 {
                    cuts.push(p.fraction);
                }
            }
            cuts.sort_by(f64::total_cmp);
            cuts.dedup();
            for pair in cuts.windows(2) {
                let weight = (cost as f64 * pair[1]).round() as u64 - (cost as f64 * pair[0]).round() as u64;
                edges.push((
                    id as u32,
                    vertex(graph, Position { road: id as u32, fraction: pair[0] }),
                    vertex(graph, Position { road: id as u32, fraction: pair[1] }),
                    weight,
                ));
            }
        }
        let mut distance = vec![u64::MAX; edges.len()];
        let mut heap = BinaryHeap::new();
        for (id, edge) in edges.iter().enumerate() {
            if edge.1 == source {
                distance[id] = edge.3;
                heap.push(Reverse((edge.3, id)));
            }
        }
        while let Some(Reverse((cost, id))) = heap.pop() {
            if distance[id] != cost {
                continue;
            }
            let edge = edges[id];
            if edge.2 == target {
                return Some(cost);
            }
            for (next, out) in edges.iter().enumerate() {
                if edge.2 != out.1 {
                    continue;
                }
                if matches!(edge.2, Vertex::Junction(_)) && !graph.permits_turn(edge.0, out.0, profile.walking) {
                    continue;
                }
                let value = cost + out.3;
                if value < distance[next] {
                    distance[next] = value;
                    heap.push(Reverse((value, next)));
                }
            }
        }
        None
    }

    #[test]
    fn attachments_match_explicit_subdivision_for_every_fixture_pair() {
        let graph = graph();
        let profile = profile();
        let positions: Vec<_> = (0..graph.roads.len() as u32)
            .flat_map(|road| [0.0, 0.25, 0.7, 1.0].map(|fraction| Position { road, fraction }))
            .collect();
        for &start in &positions {
            for &end in &positions {
                let result =
                    route_via(&graph, &profile, &[one(start), one(end)], |a, b| core(&graph, &profile, a, b)).unwrap();
                assert_eq!(
                    result.as_ref().map(|r| r.cost),
                    split_oracle(&graph, &profile, start, end),
                    "{start:?}->{end:?}"
                );
                if let Some(route) = result {
                    let cost: u64 = route
                        .slices
                        .iter()
                        .map(|s| {
                            prefix_cost(&graph, &profile, Position { road: s.road, fraction: s.to }).unwrap()
                                - prefix_cost(&graph, &profile, Position { road: s.road, fraction: s.from }).unwrap()
                        })
                        .sum();
                    assert_eq!(cost, route.cost);
                }
            }
        }
    }

    #[test]
    fn a_shape_point_preserves_direction_and_restriction_history() {
        let graph = graph();
        let profile = profile();
        let points = [
            one(Position { road: 0, fraction: 0.8 }),
            one(Position { road: 0, fraction: 1.0 }),
            one(Position { road: 2, fraction: 0.2 }),
        ];
        let route = route_via(&graph, &profile, &points, |a, b| core(&graph, &profile, a, b)).unwrap().unwrap();
        assert!(route.cost > 40);
        for pair in route.slices.windows(2) {
            assert!(graph.permits_turn(pair[0].road, pair[1].road, false));
        }
        let forward = [
            one(Position { road: 8, fraction: 0.2 }),
            one(Position { road: 8, fraction: 0.5 }),
            one(Position { road: 8, fraction: 0.8 }),
        ];
        assert_eq!(
            route_via(&graph, &profile, &forward, |a, b| core(&graph, &profile, a, b)).unwrap().unwrap().cost,
            60
        );
        let backward = [
            one(Position { road: 8, fraction: 0.8 }),
            one(Position { road: 8, fraction: 0.5 }),
            one(Position { road: 8, fraction: 0.2 }),
        ];
        assert!(route_via(&graph, &profile, &backward, |a, b| core(&graph, &profile, a, b)).unwrap().is_none());
    }

    #[test]
    fn nearest_band_mode_filter_and_crossing_provenance_are_explicit() {
        let mut graph = graph();
        let profile = profile();
        let p = Point { lat: 0, lon: 500, elevation: 0 };
        let index = SpatialIndex::around(&graph, &[p], 30.0).unwrap();
        let policy = Policy { radius_m: 30.0, ambiguity_m: 1.0, max_candidates: 8 };
        let candidates = index.candidates(&graph, &profile, p, policy).unwrap();
        assert_eq!(candidates.retained.len(), 3);
        assert!(candidates.retained.iter().all(|c| (c.position.fraction - 0.5).abs() < 1e-9));
        let isolated = one(Position { road: 8, fraction: 0.5 });
        assert!(route_via(&graph, &profile, &[one(Position { road: 0, fraction: 0.4 }), isolated], |a, b| core(
            &graph, &profile, a, b
        ))
        .unwrap()
        .is_none());
        graph.roads[8].access = FOOT;
        let capped = index.candidates(&graph, &profile, p, Policy { max_candidates: 1, ..policy }).unwrap();
        assert_eq!(capped.eligible_roads_in_radius, 2);
        assert!(capped.truncated);
        assert_eq!(capped.retained[0].position.road, 0);
        let far = Point { lat: 20_000, ..p };
        assert!(index.candidates(&graph, &profile, far, policy).is_err());
    }

    #[test]
    fn partial_climb_cost_follows_the_samples_and_integer_pieces_telescope() {
        let mut graph = graph();
        let mut profile = profile();
        profile.climb_weight = 10.0;
        graph.roads[0].shape = vec![
            Point { lat: 0, lon: 0, elevation: 0 },
            Point { lat: 0, lon: 500, elevation: 0 },
            Point { lat: 0, lon: 1000, elevation: 10 },
        ];
        graph.roads[0].ascent_m = 10;
        assert_eq!(profile.cost(&graph.roads[0]), Some(200));
        assert_eq!(prefix_cost(&graph, &profile, Position { road: 0, fraction: 0.5 }).unwrap(), 50);
        assert_eq!(prefix_cost(&graph, &profile, Position { road: 0, fraction: 0.75 }).unwrap(), 125);
        let cuts = [0.0, 0.123, 0.501, 0.789, 1.0];
        let total: u64 = cuts
            .windows(2)
            .map(|p| {
                prefix_cost(&graph, &profile, Position { road: 0, fraction: p[1] }).unwrap()
                    - prefix_cost(&graph, &profile, Position { road: 0, fraction: p[0] }).unwrap()
            })
            .sum();
        assert_eq!(total, 200);
    }
}
