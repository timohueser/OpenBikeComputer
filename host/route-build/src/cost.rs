//! Compile source attributes and terrain into additive costs before contraction.
use crate::road_bike::{self, WayCost};
use route_engine::{
    cost::RoadCost,
    model::{Graph, Point, Profile, RoadBike, Weighting, BIKE, FOOT, NO_ELEVATION, PUSH},
    osm::Id,
};
use std::collections::HashSet;

pub struct Costing<'a> {
    graph: &'a Graph,
    profile: &'a Profile,
    ways: Vec<Option<WayCost>>,
    pub roads: Vec<Option<RoadCost>>,
}

impl<'a> Costing<'a> {
    pub fn new(graph: &'a Graph, profile: &'a Profile) -> Result<Self, String> {
        profile.validate()?;
        if graph.node_access.len() != graph.points.len() || graph.node_ids.len() != graph.points.len() {
            return Err("Graph nodes lack source identities or access".into());
        }
        let mut cycle_routes = HashSet::new();
        let mut pending: Vec<_> = graph
            .osm
            .relations
            .values()
            .filter(|relation| {
                road_bike::tag(&relation.tags, "route") == "bicycle"
                    && road_bike::tag(&relation.tags, "state") != "proposed"
            })
            .map(|r| r.id)
            .collect();
        let mut visited = HashSet::new();
        while let Some(id) = pending.pop() {
            if !visited.insert(id) {
                continue;
            }
            let Some(relation) = graph.osm.relations.get(&id) else {
                continue;
            };
            if road_bike::tag(&relation.tags, "state") == "proposed" {
                continue;
            }
            for (id, _) in &relation.members {
                match id {
                    Id::Way(id) => {
                        cycle_routes.insert(*id);
                    }
                    Id::Relation(id) => pending.push(*id),
                    _ => {}
                }
            }
        }
        let mut ways = Vec::with_capacity(graph.roads.len());
        let mut roads = Vec::with_capacity(graph.roads.len());
        for road in &graph.roads {
            if road.shape.len() < 2 {
                return Err("Road has no geometry".into());
            }
            let way = if !profile.permits(road) {
                None
            } else {
                match &profile.weighting {
                    Weighting::RoadBike(variant) => {
                        let source = graph.osm.ways.get(&road.way).ok_or("Road lacks source OSM way")?;
                        road_bike::way(road, &source.tags, *variant, cycle_routes.contains(&road.way))
                    }
                    Weighting::Weighted { surface, road: weights, .. } => Some(WayCost {
                        factor: surface[road.surface as usize]
                            * *weights.get(road.class as usize).ok_or("Unknown road class")?
                            * if !profile.walking && road.access & BIKE == 0 { 4.0 } else { 1.0 },
                        turn: 0.0,
                        ferry: road.class == 6,
                        pushing: !profile.walking && road.access & BIKE == 0,
                    }),
                }
            };
            roads.push(way.map(|w| curve(road, profile, w)).transpose()?);
            ways.push(way);
        }
        Ok(Self { graph, profile, ways, roads })
    }

    pub fn transition(&self, before: u32, after: u32) -> Option<u64> {
        let (a, b) = (&self.graph.roads[before as usize], &self.graph.roads[after as usize]);
        if a.to != b.from || !self.graph.permits_turn(before, after, self.profile.walking) {
            return None;
        }
        let access = self.graph.node_access[b.from as usize];
        let allowed = if self.profile.walking {
            access & FOOT != 0
        } else {
            access & BIKE != 0 || self.profile.pushing && access & PUSH != 0
        };
        if !allowed {
            return None;
        }
        let (wa, wb) = (self.ways[before as usize]?, self.ways[after as usize]?);
        if (wa.pushing || wb.pushing || access & BIKE == 0) && !self.graph.permits_turn(before, after, true) {
            return None;
        }
        if !matches!(self.profile.weighting, Weighting::RoadBike(_)) {
            return Some(0);
        }
        let mut cost = turn(a.shape[a.shape.len() - 2], a.shape[a.shape.len() - 1], b.shape[1], wb.turn);
        if self.graph.osm.nodes.get(&self.graph.node_ids[b.from as usize]).is_some_and(|node| {
            road_bike::tag(&node.tags, "highway") == "traffic_signals"
                || road_bike::tag(&node.tags, "highway") == "crossing"
                    && road_bike::tag(&node.tags, "crossing") == "traffic_signals"
        }) {
            cost += 20.0;
        }
        if wb.ferry && !wa.ferry {
            cost += 10_000.0;
        }
        if !wa.pushing && (wb.pushing || access & BIKE == 0) {
            cost += 300.0;
        }
        Some(cost.round() as u64)
    }
}

fn curve(road: &route_engine::model::Road, profile: &Profile, way: WayCost) -> Result<RoadCost, String> {
    let lengths: Vec<_> = road.shape.windows(2).map(|p| p[0].distance(p[1])).collect();
    let total: f64 = lengths.iter().sum();
    if total <= 0.0 {
        return Err("Road has zero geometric length".into());
    }
    let (up, down, cutoff) = match profile.weighting {
        Weighting::RoadBike(RoadBike::Shorter) => (0.0, 0.0, 0.015),
        Weighting::RoadBike(RoadBike::LessClimbing) => (60.0, 60.0, 0.015),
        Weighting::RoadBike(_) => (0.0, 60.0, 0.015),
        Weighting::Weighted { climb, .. } => (climb, 0.0, 0.0),
    };
    let mut result = RoadCost { distance: road.length_m as f64 * way.factor, penalties: Vec::new() };
    let mut distance = 0.0;
    let mut penalty = 0.0;
    for (i, (&length, pair)) in lengths.iter().zip(road.shape.windows(2)).enumerate() {
        if i > 0 && way.turn != 0.0 {
            let bend = turn(road.shape[i - 1], road.shape[i], road.shape[i + 1], way.turn);
            if bend > 0.0 {
                penalty += bend;
                knot(&mut result.penalties, distance / total, penalty);
            }
        }
        if pair.iter().all(|p| p.elevation != NO_ELEVATION) && !way.ferry {
            let delta = pair[1].elevation as f64 - pair[0].elevation as f64;
            penalty += (delta - length * cutoff).max(0.0) * up + (-delta - length * cutoff).max(0.0) * down;
        }
        distance += length;
        knot(&mut result.penalties, (distance / total).min(1.0), penalty);
    }
    if penalty == 0.0 {
        result.penalties.clear();
    }
    if !result.valid() {
        return Err("Invalid prepared road cost".into());
    }
    Ok(result)
}

fn knot(points: &mut Vec<(f64, f64)>, fraction: f64, cost: f64) {
    let next = (fraction, cost);
    if let Some(&last) = points.last() {
        let previous = if points.len() > 1 { points[points.len() - 2] } else { (0.0, 0.0) };
        // Keep changes of slope and steps, but omit collinear terrain samples.
        if fraction > last.0
            && last.0 > previous.0
            && ((next.1 - last.1) * (last.0 - previous.0) - (last.1 - previous.1) * (next.0 - last.0)).abs() < 1e-9
        {
            points.pop();
        }
    }
    points.push(next);
}

fn turn(a: Point, b: Point, c: Point, base: f64) -> f64 {
    let scale = (b.lat as f64 * 1e-6).to_radians().cos();
    let ab = ((b.lon as f64 - a.lon as f64) * scale, b.lat as f64 - a.lat as f64);
    let bc = ((c.lon as f64 - b.lon as f64) * scale, c.lat as f64 - b.lat as f64);
    let lengths = ab.0.hypot(ab.1) * bc.0.hypot(bc.1);
    if lengths == 0.0 {
        return 0.0;
    }
    let cosine = ((ab.0 * bc.0 + ab.1 * bc.1) / lengths).clamp(-1.0, 1.0);
    ((1.0 - cosine) * base + 0.2).floor()
}

#[cfg(test)]
mod tests {
    use super::*;
    use route_engine::model::{Road, Surface};

    fn road(shape: Vec<Point>) -> Road {
        Road {
            from: 0,
            to: 1,
            way: 1,
            reversed: false,
            length_m: shape.windows(2).map(|p| p[0].distance(p[1])).sum::<f64>().round() as u32,
            ascent_m: 0,
            descent_m: 0,
            surface: Surface::Paved,
            class: 1,
            access: BIKE,
            difficulty: 255,
            hiking_difficulty: None,
            uncertain_access: false,
            structure: false,
            shape,
        }
    }
    fn cost(road: &Road, name: &str) -> RoadCost {
        let profile = Profile::presets().into_iter().find(|p| p.name == name).unwrap();
        curve(road, &profile, WayCost { factor: 1.0, turn: 90.0, ferry: false, pushing: false }).unwrap()
    }
    fn point(lon: i32, elevation: f32) -> Point {
        Point { lat: 0, lon, elevation }
    }

    #[test]
    fn elevation_threshold_is_directional_and_invariant_to_linear_sampling() {
        let gentle = road(vec![point(0, 100.0), point(10_000, 90.0)]);
        assert_eq!(cost(&gentle, "road").total(), gentle.length_m as u64);
        let descent = road(vec![point(0, 100.0), point(10_000, 0.0)]);
        let expected = (descent.length_m as f64 + (100.0 - 1111.95 * 0.015) * 60.0).round() as u64;
        assert_eq!(cost(&descent, "road").total(), expected);
        let dense = road((0..=100).map(|i| point(i * 100, 100.0 - i as f32)).collect());
        assert_eq!(cost(&dense, "road").total(), expected);
        let ascent = road(descent.shape.iter().rev().copied().collect());
        assert_eq!(cost(&ascent, "road").total(), ascent.length_m as u64);
        assert_eq!(cost(&ascent, "road/less-climbing").total(), expected);
        assert_eq!(cost(&descent, "road/shorter").total(), descent.length_m as u64);
        let unknown = road(vec![point(0, NO_ELEVATION), point(10_000, 0.0)]);
        assert_eq!(cost(&unknown, "road").total(), unknown.length_m as u64);
    }

    #[test]
    fn pushing_obeys_pedestrian_turn_restrictions_on_roads_and_at_crossings() {
        let points = vec![point(0, 0.0), point(1000, 0.0), point(2000, 0.0)];
        let mut graph = Graph {
            node_ids: vec![0, 1, 2],
            node_access: vec![BIKE | FOOT | PUSH; 3],
            roads: vec![road(points[..2].to_vec()), Road { from: 1, to: 2, ..road(points[1..].to_vec()) }],
            points,
            forbidden_foot: vec![(0, 1)],
            ..Graph::default()
        };
        graph.osm.ways.insert(
            1,
            route_engine::osm::Way {
                id: 1,
                nodes: vec![0, 1, 2],
                tags: [("highway".into(), "residential".into())].into_iter().collect(),
            },
        );
        for profile in Profile::presets().into_iter().filter(|p| ["road", "touring"].contains(&p.name.as_str())) {
            assert!(Costing::new(&graph, &profile).unwrap().transition(0, 1).is_some());
            for edge in 0..2 {
                graph.roads[edge].access = FOOT | PUSH;
                assert!(Costing::new(&graph, &profile).unwrap().transition(0, 1).is_none());
                graph.roads[edge].access = BIKE;
            }
            graph.node_access[1] = FOOT | PUSH;
            assert!(Costing::new(&graph, &profile).unwrap().transition(0, 1).is_none());
            graph.node_access[1] = BIKE | FOOT | PUSH;
        }
    }

    #[test]
    fn a_bend_costs_the_same_inside_a_way_and_at_a_junction() {
        let a = point(0, 0.0);
        let b = point(1000, 0.0);
        let c = Point { lat: 1000, ..b };
        let bent = road(vec![a, b, c]);
        let compiled = cost(&bent, "road");
        assert_eq!(turn(a, b, c, 90.0), 90.0);
        assert_eq!(compiled.total(), bent.length_m as u64 + 90);
        let before = compiled.prefix(0.49).unwrap();
        let after = compiled.prefix(0.5).unwrap();
        assert!((92..=93).contains(&(after - before)));
        let prefixes: Vec<_> = [0.0, 0.2, 0.49, 0.5, 0.9, 1.0].iter().map(|&f| compiled.prefix(f).unwrap()).collect();
        assert_eq!(prefixes.windows(2).map(|p| p[1] - p[0]).sum::<u64>(), compiled.total());
    }
}
