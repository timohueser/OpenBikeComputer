//! Compile source attributes and terrain into additive routing costs.
use crate::road_bike::{self, WayCost};
use route_engine::{
    closures::Closures,
    cost::{turn, CostBasis, CostParameters, RoadCost},
    model::{Graph, Profile, Weighting, BIKE, FOOT, PUSH},
    osm::Id,
};
use std::collections::HashMap;

/// A road that the rider may have no access to costs three times its length. An ordinary route
/// takes it only where every other way is more than twice as long as the road; a shaping point
/// on the road still takes the route through it.
const UNCERTAIN_ACCESS: f64 = 3.0;

pub struct Costing<'a> {
    graph: &'a Graph,
    profile: &'a Profile,
    ways: Vec<Option<WayCost>>,
    pub roads: Vec<Option<RoadCost>>,
}

impl<'a> Costing<'a> {
    pub fn new(graph: &'a Graph, profile: &'a Profile, closures: &Closures) -> Result<Self, String> {
        profile.validate()?;
        if graph.node_access.len() != graph.points.len() || graph.node_ids.len() != graph.points.len() {
            return Err("Graph nodes lack source identities or access".into());
        }
        let cycle_routes = cycle_routes(graph);
        let mut ways = Vec::with_capacity(graph.roads.len());
        let mut roads = Vec::with_capacity(graph.roads.len());
        for (id, road) in graph.roads.iter().enumerate() {
            if road.shape.len() < 2 {
                return Err("Road has no geometry".into());
            }
            let way = if !profile.permits(road) {
                None
            } else {
                match &profile.weighting {
                    Weighting::RoadBike(variant) => {
                        let source = graph.osm.ways.get(&road.way).ok_or("Road lacks source OSM way")?;
                        road_bike::way(road, &source.tags, *variant, cycle_routes.contains_key(&road.way))
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
            let way = way.map(|mut cost| {
                let mode = if profile.walking {
                    FOOT
                } else if road.access & BIKE != 0 {
                    BIKE
                } else {
                    PUSH
                };
                if closures.closing(id as u32, mode).is_some_and(|found| found.iter().any(|c| c.kind.avoided())) {
                    cost.factor *= UNCERTAIN_ACCESS;
                }
                if !profile.walking && !cost.pushing && !cost.ferry && !profile.name.ends_with("/shorter") {
                    if let Some(&rank) = cycle_routes.get(&road.way) {
                        let touring = profile.name.split('/').next() == Some("touring");
                        cost.factor *= 1.0 - f64::from(rank) * if touring { 0.1 } else { 0.04 };
                    }
                }
                cost
            });
            roads.push(way.map(|w| parameters(road, w).compile(road, profile)).transpose()?);
            ways.push(way);
        }
        Ok(Self { graph, profile, ways, roads })
    }

    pub fn basis(&self, road: usize) -> Option<CostBasis> {
        self.ways[road].map(|way| CostBasis { factor: way.factor, turn: way.turn, ferry: way.ferry })
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

fn parameters(road: &route_engine::model::Road, way: WayCost) -> CostParameters {
    CostParameters { distance: road.length_m as f64 * way.factor, turn: way.turn, ferry: way.ferry }
}

fn cycle_routes(graph: &Graph) -> HashMap<i64, u8> {
    let mut pending: Vec<_> = graph
        .osm
        .relations
        .values()
        .filter_map(|relation| {
            let tags = &relation.tags;
            if road_bike::tag(tags, "route") != "bicycle"
                || !matches!(road_bike::tag(tags, "type"), "route" | "superroute")
            {
                return None;
            }
            let rank = match road_bike::tag(tags, "network") {
                "icn" => 5,
                "ncn" => 4,
                "rcn" => 3,
                "lcn" => 2,
                _ => 1,
            };
            Some((relation.id, rank))
        })
        .collect();
    let mut visited = HashMap::<i64, u8>::new();
    let mut ways = HashMap::<i64, u8>::new();
    while let Some((id, rank)) = pending.pop() {
        if visited.get(&id).is_some_and(|previous| *previous >= rank) {
            continue;
        }
        visited.insert(id, rank);
        let Some(relation) = graph.osm.relations.get(&id) else {
            continue;
        };
        if road_bike::tag(&relation.tags, "state") == "proposed" {
            continue;
        }
        for (member, _) in &relation.members {
            match member {
                Id::Way(id) => {
                    ways.entry(*id).and_modify(|value| *value = (*value).max(rank)).or_insert(rank);
                }
                Id::Relation(id) => pending.push((*id, rank)),
                _ => {}
            }
        }
    }
    ways
}

#[cfg(test)]
mod tests {
    use super::*;
    use route_engine::model::{Point, Road, Surface, NO_ELEVATION};

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
        parameters(road, WayCost { factor: 1.0, turn: 90.0, ferry: false, pushing: false })
            .compile(road, &profile)
            .unwrap()
    }
    fn point(lon: i32, elevation: f32) -> Point {
        Point { lat: 0, lon, elevation }
    }

    #[test]
    fn cycling_networks_favour_touring_by_level_without_discounting_walks_or_pushing() {
        use route_engine::osm::{Relation, Way};
        let shape = vec![point(0, 0.0), point(1000, 0.0)];
        let mut graph = Graph {
            points: shape.clone(),
            node_ids: vec![0, 1],
            node_access: vec![BIKE | FOOT | PUSH; 2],
            roads: vec![Road { access: BIKE | FOOT | PUSH, ..road(shape) }],
            ..Graph::default()
        };
        graph.osm.ways.insert(
            1,
            Way { id: 1, nodes: vec![0, 1], tags: [("highway".into(), "residential".into())].into_iter().collect() },
        );
        let profiles = Profile::presets();
        let cost = |graph: &Graph, name: &str| {
            let profile = profiles.iter().find(|p| p.name == name).unwrap();
            Costing::new(graph, profile, &Closures::default()).unwrap().roads[0].as_ref().unwrap().total()
        };
        let ordinary = cost(&graph, "touring");
        let walking = cost(&graph, "hiking");
        let mut previous = ordinary;
        for network in ["", "lcn", "rcn", "ncn", "icn"] {
            graph.osm.relations.insert(
                10,
                Relation {
                    id: 10,
                    tags: [
                        ("type".into(), "route".into()),
                        ("route".into(), "bicycle".into()),
                        ("network".into(), network.into()),
                    ]
                    .into_iter()
                    .collect(),
                    members: vec![(Id::Way(1), String::new())],
                },
            );
            let preferred = cost(&graph, "touring");
            assert!(preferred < previous);
            assert_eq!(cost(&graph, "hiking"), walking);
            previous = preferred;
        }
        graph.osm.relations.insert(
            20,
            Relation {
                id: 20,
                tags: [
                    ("type".into(), "superroute".into()),
                    ("route".into(), "bicycle".into()),
                    ("network".into(), "icn".into()),
                ]
                .into_iter()
                .collect(),
                members: vec![(Id::Relation(10), String::new())],
            },
        );
        graph.osm.relations.get_mut(&10).unwrap().tags.insert("network".into(), "lcn".into());
        graph.osm.relations.get_mut(&10).unwrap().members.push((Id::Relation(20), String::new()));
        assert_eq!(cost(&graph, "touring"), previous);
        graph.osm.relations.get_mut(&10).unwrap().tags.insert("state".into(), "proposed".into());
        assert_eq!(cost(&graph, "touring"), ordinary);
        graph.roads[0].access = FOOT | PUSH;
        let pushing = cost(&graph, "touring");
        graph.osm.relations.clear();
        assert_eq!(cost(&graph, "touring"), pushing);
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
            assert!(Costing::new(&graph, &profile, &Closures::default()).unwrap().transition(0, 1).is_some());
            for edge in 0..2 {
                graph.roads[edge].access = FOOT | PUSH;
                assert!(Costing::new(&graph, &profile, &Closures::default()).unwrap().transition(0, 1).is_none());
                graph.roads[edge].access = BIKE;
            }
            graph.node_access[1] = FOOT | PUSH;
            assert!(Costing::new(&graph, &profile, &Closures::default()).unwrap().transition(0, 1).is_none());
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
