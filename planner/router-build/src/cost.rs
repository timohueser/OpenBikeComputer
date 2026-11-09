//! Compile source attributes and terrain into additive routing costs.
use crate::{road_bike, source::Id, suitability, Graph};
use planner_router::{
    closures::Closure,
    cost::{turn, CostBasis, RoadCost},
    model::{Profile, Road, Weighting, BIKE, FOOT, PUSH},
};
use std::collections::HashMap;

/// A road that the rider may have no access to costs three times its length. An ordinary route
/// takes it only where every other way is more than three times as long as the road; a shaping
/// point on the road still takes the route through it.
const UNCERTAIN_ACCESS: f64 = 3.0;

/// Passing a node that the rider may have no access to, such as a private gate, costs as much as
/// 300 m of good road. A factor on the arriving road would cost nothing where the gate is near a
/// junction.
const UNCERTAIN_NODE: f64 = 300.0;

/// The modes that an access value makes doubtful, on each road and at each node. Seasonal and
/// conditional closures depend on the ride, so they are not doubts.
#[derive(Default)]
pub struct Doubts {
    pub roads: Vec<u8>,
    pub nodes: HashMap<u32, u8>,
}

impl Doubts {
    pub fn modes(closures: &[(u8, Closure)]) -> u8 {
        closures.iter().filter(|(_, closure)| closure.kind.avoided()).fold(0, |modes, (bits, _)| modes | bits)
    }
}

pub struct Costing<'a> {
    graph: &'a Graph,
    profile: &'a Profile,
    doubts: &'a Doubts,
    /// The exact factors each road's curve compiles from; `None` where the profile excludes it.
    pub bases: Vec<Option<CostBasis>>,
    pub roads: Vec<Option<RoadCost>>,
    /// The roads of a transition, reused.
    pair: [Road; 2],
}

impl<'a> Costing<'a> {
    pub fn new(graph: &'a Graph, profile: &'a Profile, doubts: &'a Doubts) -> Result<Self, String> {
        profile.validate()?;
        if graph.node_access.len() != graph.points.len() || graph.node_ids.len() != graph.points.len() {
            return Err("Graph nodes lack source identities or access".into());
        }
        let cycle_routes = if profile.route_bonus == 0.0 { HashMap::new() } else { cycle_routes(graph) };
        let mut bases = Vec::with_capacity(graph.roads.len());
        let mut roads = Vec::with_capacity(graph.roads.len());
        let mut road = Road::default();
        for id in 0..graph.roads.len() {
            graph.roads.get(id, &mut road);
            let road = &road;
            if road.shape.len() < 2 {
                return Err("Road has no geometry".into());
            }
            let mode = profile.mode(road);
            let suitability = if !profile.permits(road) {
                None
            } else {
                let source = graph.osm.ways.get(road.way);
                let country = source.as_ref().map(|source| graph.osm.country(source)).unwrap_or_default();
                match &profile.weighting {
                    Weighting::RoadBike(variant) => {
                        let source = source.ok_or("Road lacks source OSM way")?;
                        road_bike::way(road, &source.tags, country, *variant, mode == PUSH)
                    }
                    Weighting::Weighted { .. } => {
                        let empty = Default::default();
                        suitability::way(road, source.as_ref().map_or(&empty, |source| &source.tags), country, profile)
                    }
                }
            };
            let basis = suitability.map(|suitability| {
                let mut cost = suitability.basis;
                // A signed route discounts only the preference above the physical and traffic floor.
                if mode == BIKE && !cost.ferry {
                    if let Some(ranks) = cycle_routes.get(&road.way) {
                        let preference = (cost.factor - suitability.minimum_factor).max(0.0);
                        cost.factor = suitability.minimum_factor
                            + preference * (1.0 - f64::from(ranks[usize::from(road.reversed)]) * profile.route_bonus);
                    }
                }
                if doubts.roads.get(id).is_some_and(|&modes| modes & mode != 0) {
                    cost.factor *= UNCERTAIN_ACCESS;
                }
                cost
            });
            roads.push(basis.map(|basis| basis.compile(road, profile)).transpose()?);
            bases.push(basis);
        }
        Ok(Self { graph, profile, doubts, bases, roads, pair: Default::default() })
    }

    pub fn transition(&mut self, before: u32, after: u32) -> Option<u64> {
        let [a, b] = &mut self.pair;
        self.graph.roads.get(before as usize, a);
        self.graph.roads.get(after as usize, b);
        let (a, b) = (&*a, &*b);
        if a.to != b.from || !self.graph.permits_turn(before, after, self.profile.walking) {
            return None;
        }
        // A U-turn onto the reverse of the same road does not pass the node, so a closed gate at
        // the end of a spur still lets the rider turn back; its access and doubt do not apply.
        let passes = !(a.way == b.way && a.from == b.to && a.to == b.from);
        let access = if passes { self.graph.node_access[b.from as usize] } else { BIKE | FOOT | PUSH };
        if access & self.profile.modes() == 0 {
            return None;
        }
        let (wa, wb) = (self.bases[before as usize]?, self.bases[after as usize]?);
        let pushing = |road: &Road| self.profile.mode(road) == PUSH;
        if (pushing(a) || pushing(b) || access & BIKE == 0) && !self.graph.permits_turn(before, after, true) {
            return None;
        }
        let doubt = passes && self.doubts.nodes.get(&b.from).is_some_and(|&modes| modes & self.profile.mode(b) != 0);
        let doubt = if doubt { UNCERTAIN_NODE } else { 0.0 };
        if !matches!(self.profile.weighting, Weighting::RoadBike(_)) {
            return Some(doubt as u64);
        }
        let mut cost = doubt + turn(a.shape[a.shape.len() - 2], a.shape[a.shape.len() - 1], b.shape[1], wb.turn);
        if self.graph.osm.nodes.get(self.graph.node_ids[b.from as usize]).is_some_and(|node| {
            road_bike::tag(&node.tags, "highway") == "traffic_signals"
                || road_bike::tag(&node.tags, "highway") == "crossing"
                    && road_bike::tag(&node.tags, "crossing") == "traffic_signals"
        }) {
            cost += 20.0;
        }
        if wb.ferry && !wa.ferry {
            cost += 10_000.0;
        }
        if !pushing(a) && (pushing(b) || access & BIKE == 0) {
            cost += 300.0;
        }
        Some(cost.round() as u64)
    }
}

/// The highest route rank of each way, forward and backward.
fn cycle_routes(graph: &Graph) -> HashMap<i64, [u8; 2]> {
    let mut pending: Vec<_> = graph
        .osm
        .relations
        .iter()
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
    let mut ways = HashMap::<i64, [u8; 2]>::new();
    while let Some((id, rank)) = pending.pop() {
        if visited.get(&id).is_some_and(|previous| *previous >= rank) {
            continue;
        }
        visited.insert(id, rank);
        let Some(relation) = graph.osm.relations.get(id) else {
            continue;
        };
        if road_bike::tag(&relation.tags, "state") == "proposed" {
            continue;
        }
        for (member, role) in relation.members() {
            match member {
                Id::Way(id) => {
                    // A `forward` or `backward` role follows the way's node order.
                    let ranks = ways.entry(id).or_default();
                    for (reversed, value) in ranks.iter_mut().enumerate() {
                        if role != if reversed == 0 { "backward" } else { "forward" } {
                            *value = (*value).max(rank);
                        }
                    }
                }
                Id::Relation(id) => pending.push((id, rank)),
                _ => {}
            }
        }
    }
    ways
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::{Builder, Relations, Table, Tags, Ways};
    use planner_router::model::{Point, Road, Surface, NO_ELEVATION};

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
            structure: false,
            shape,
        }
    }
    fn cost(road: &Road, name: &str) -> RoadCost {
        let profile = Profile::presets().into_iter().find(|p| p.name == name).unwrap();
        CostBasis { factor: 1.0, turn: 90.0, ferry: false }.compile(road, &profile).unwrap()
    }
    fn point(lon: i32, elevation: f32) -> Point {
        Point { lat: 0, lon, elevation }
    }

    /// Way 1.
    fn way(nodes: &[i64], tags: &[(&str, &str)]) -> Table<Ways> {
        let mut ways = Builder::<Ways>::default();
        ways.push(1, nodes, &Tags::from_pairs(tags.iter().copied()));
        ways.finish().0
    }

    type Route<'a> = (i64, Vec<(&'a str, &'a str)>, Vec<(Id, &'a str)>);

    fn relations(routes: &[&Route]) -> Table<Relations> {
        let mut relations = Builder::<Relations>::default();
        for (id, tags, members) in routes {
            relations.push(*id, &Tags::from_pairs(tags.iter().copied()), members.iter().copied());
        }
        relations.finish().0
    }

    #[test]
    fn cycling_networks_favour_touring_by_level_without_discounting_walks_or_pushing() {
        let shape = vec![point(0, 0.0), point(1000, 0.0)];
        let template = Road { access: BIKE | FOOT | PUSH, ..road(shape.clone()) };
        let mut graph = Graph {
            points: shape,
            node_ids: vec![0, 1],
            node_access: vec![BIKE | FOOT | PUSH; 2],
            roads: [template.clone()].into_iter().collect(),
            ..Graph::default()
        };
        let set = |graph: &mut Graph, road: Road| graph.roads = [road].into_iter().collect();
        graph.osm.ways = way(&[0, 1], &[("highway", "residential")]);
        let profiles = Profile::presets();
        let cost = |graph: &Graph, name: &str| {
            let profile = profiles.iter().find(|p| p.name == name).unwrap();
            Costing::new(graph, profile, &Doubts::default()).unwrap().roads[0].as_ref().unwrap().total()
        };
        let ordinary = cost(&graph, "touring");
        let walking = cost(&graph, "hiking");
        let mut previous = ordinary;
        let route = |network| vec![("type", "route"), ("route", "bicycle"), ("network", network)];
        let mut local: Route = (10, route(""), vec![(Id::Way(1), "")]);
        for network in ["", "lcn", "rcn", "ncn", "icn"] {
            local.1 = route(network);
            graph.osm.relations = relations(&[&local]);
            let preferred = cost(&graph, "touring");
            assert!(preferred < previous);
            assert_eq!(cost(&graph, "hiking"), walking);
            previous = preferred;
        }
        let parent =
            (20, vec![("type", "superroute"), ("route", "bicycle"), ("network", "icn")], vec![(Id::Relation(10), "")]);
        local.1 = route("lcn");
        local.2.push((Id::Relation(20), ""));
        graph.osm.relations = relations(&[&local, &parent]);
        assert_eq!(cost(&graph, "touring"), previous);
        local.1.push(("state", "proposed"));
        graph.osm.relations = relations(&[&local, &parent]);
        assert_eq!(cost(&graph, "touring"), ordinary);
        local.1.pop();
        for role in ["forward", "backward"] {
            local.2[0].1 = role;
            graph.osm.relations = relations(&[&local, &parent]);
            for reversed in [false, true] {
                set(&mut graph, Road { reversed, ..template.clone() });
                let against = role == if reversed { "forward" } else { "backward" };
                assert_eq!(cost(&graph, "touring") == ordinary, against, "{role}, reversed {reversed}");
            }
        }
        set(&mut graph, template.clone());
        local.2[0].1 = "";
        graph.osm.relations = relations(&[&local, &parent]);
        // The discount stops at the traffic floor: a signed main road never undercuts a quiet road.
        let quiet = cost(&graph, "touring");
        set(&mut graph, Road { class: 2, ..template.clone() });
        graph.osm.ways = way(&[0, 1], &[("highway", "primary"), ("maxspeed", "90")]);
        assert!(cost(&graph, "touring") > quiet && cost(&graph, "road") > quiet);
        graph.osm.ways = way(&[0, 1], &[("highway", "residential")]);
        set(&mut graph, Road { access: FOOT | PUSH, ..template });
        let pushing = cost(&graph, "touring");
        graph.osm.relations = Table::default();
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
        let roads = [road(points[..2].to_vec()), Road { from: 1, to: 2, ..road(points[1..].to_vec()) }];
        let mut graph = Graph {
            node_ids: vec![0, 1, 2],
            node_access: vec![BIKE | FOOT | PUSH; 3],
            roads: roads.iter().cloned().collect(),
            points,
            forbidden_foot: vec![(0, 1)],
            ..Graph::default()
        };
        graph.osm.ways = way(&[0, 1, 2], &[("highway", "residential")]);
        for profile in Profile::presets().into_iter().filter(|p| ["road", "touring"].contains(&p.name.as_str())) {
            assert!(Costing::new(&graph, &profile, &Doubts::default()).unwrap().transition(0, 1).is_some());
            for edge in 0..2 {
                let mut pushed = roads.clone();
                pushed[edge].access = FOOT | PUSH;
                graph.roads = pushed.into_iter().collect();
                assert!(Costing::new(&graph, &profile, &Doubts::default()).unwrap().transition(0, 1).is_none());
            }
            graph.roads = roads.iter().cloned().collect();
            graph.node_access[1] = FOOT | PUSH;
            assert!(Costing::new(&graph, &profile, &Doubts::default()).unwrap().transition(0, 1).is_none());
            graph.node_access[1] = BIKE | FOOT | PUSH;
        }
    }

    #[test]
    fn an_access_value_costs_a_road_three_times_and_a_node_a_fixed_charge() {
        use planner_router::closures::Kind;
        let points = vec![point(0, 0.0), point(1000, 0.0), point(2000, 0.0)];
        let mut graph = Graph {
            node_ids: vec![0, 1, 2],
            node_access: vec![BIKE | FOOT | PUSH; 3],
            roads: [road(points[..2].to_vec()), Road { from: 1, to: 2, ..road(points[1..].to_vec()) }]
                .into_iter()
                .collect(),
            points,
            ..Graph::default()
        };
        graph.osm.ways = way(&[0, 1, 2], &[("highway", "residential")]);
        let closure = |kind| (BIKE, Closure { kind, condition: String::new() });
        assert_eq!(Doubts::modes(&[closure(Kind::Seasonal), closure(Kind::Conditional)]), 0);
        assert_eq!(Doubts::modes(&[closure(Kind::Discouraged)]), BIKE);
        for name in ["touring", "road"] {
            let profile = Profile::presets().into_iter().find(|p| p.name == name).unwrap();
            let none = Doubts::default();
            let mut open = Costing::new(&graph, &profile, &none).unwrap();
            let doubts = Doubts { roads: vec![BIKE, FOOT | PUSH], nodes: [(1, BIKE)].into_iter().collect() };
            let mut doubted = Costing::new(&graph, &profile, &doubts).unwrap();
            let total = |costing: &Costing, road: usize| costing.roads[road].as_ref().unwrap().total();
            assert!(total(&doubted, 0).abs_diff(3 * total(&open, 0)) <= 2, "{name}");
            assert_eq!(total(&doubted, 1), total(&open, 1), "{name}");
            assert_eq!(doubted.transition(0, 1), open.transition(0, 1).map(|cost| cost + 300), "{name}");
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
