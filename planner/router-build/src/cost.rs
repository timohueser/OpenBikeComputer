//! Compile source attributes and terrain into additive routing costs.
use crate::{
    country::Country,
    road_bike,
    source::{Id, Tags},
    suitability, Graph,
};
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

// Initial distance-equivalent preferences: avoid repeated dismounts and short ferry shortcuts.
// These charges are not boarding waits or additions to the estimated moving time.
const DISMOUNT_ENTRY: f64 = 100.0;
const FERRY_ENTRY: f64 = 1000.0;
const WALKING_FERRY_ENTRY: f64 = 100.0;

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
}

impl<'a> Costing<'a> {
    pub fn new(graph: &'a Graph, profile: &'a Profile, doubts: &'a Doubts) -> Result<Self, String> {
        profile.validate()?;
        if graph.node_access.len() != graph.points.len() || graph.node_ids.len() != graph.points.len() {
            return Err("Graph nodes lack source identities or access".into());
        }
        let cycle_routes = if profile.route_bonus == 0.0 { HashMap::new() } else { cycle_routes(graph) };
        let mut countries = HashMap::new();
        let empty_tags = Tags::new();
        let mut bases = Vec::with_capacity(graph.roads.len());
        let mut roads = Vec::with_capacity(graph.roads.len());
        for (id, road) in graph.roads.iter().enumerate() {
            if road.shape.len() < 2 {
                return Err("Road has no geometry".into());
            }
            let mode = profile.mode(road);
            let suitability = if !profile.permits(road) {
                None
            } else {
                let source = graph.osm.ways.get(&road.way);
                let country = *countries
                    .entry(road.way)
                    .or_insert_with(|| source.map_or_else(Country::default, |source| graph.osm.country(source)));
                match &profile.weighting {
                    Weighting::RoadBike(variant) => {
                        let source = source.ok_or("Road lacks source OSM way")?;
                        road_bike::way(road, &source.tags, country, *variant, mode == PUSH)
                    }
                    Weighting::Weighted { .. } => {
                        suitability::way(road, source.map_or(&empty_tags, |source| &source.tags), country, profile)
                    }
                }
            };
            let basis = suitability.map(|suitability| {
                let mut cost = suitability.basis;
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
        Ok(Self { graph, profile, doubts, bases, roads })
    }

    pub fn transition(&self, before: u32, after: u32) -> Option<u64> {
        let (a, b) = (&self.graph.roads[before as usize], &self.graph.roads[after as usize]);
        if a.to != b.from || !self.graph.permits_turn(before, after, self.profile.walking) {
            return None;
        }
        // A U-turn onto the reverse of the same road does not pass the node, so a closed gate at
        // the end of a spur still lets the rider turn back; its access and doubt do not apply.
        let passes = !a.is_reverse_of(b);
        let access = if passes { self.graph.node_access[b.from as usize] } else { BIKE | FOOT | PUSH };
        if access & self.profile.modes() == 0 {
            return None;
        }
        let (wa, wb) = (self.bases[before as usize]?, self.bases[after as usize]?);
        let pushing = |road: &Road| self.profile.mode(road) == PUSH;
        let mode = if self.profile.walking {
            FOOT
        } else if pushing(a) || pushing(b) || access & BIKE == 0 {
            PUSH
        } else {
            BIKE
        };
        if access & mode == 0 || mode != BIKE && !self.graph.permits_turn(before, after, true) {
            return None;
        }
        let doubt = passes && self.doubts.nodes.get(&b.from).is_some_and(|&modes| modes & mode != 0);
        let mut cost = if doubt { UNCERTAIN_NODE } else { 0.0 };
        if matches!(self.profile.weighting, Weighting::RoadBike(_)) {
            cost += turn(a.shape[a.shape.len() - 2], a.shape[a.shape.len() - 1], b.shape[1], wb.turn);
            if self.graph.osm.nodes.get(&self.graph.node_ids[b.from as usize]).is_some_and(|node| {
                road_bike::tag(&node.tags, "highway") == "traffic_signals"
                    || road_bike::tag(&node.tags, "highway") == "crossing"
                        && road_bike::tag(&node.tags, "crossing") == "traffic_signals"
            }) {
                cost += 20.0;
            }
        }
        if wb.ferry && !wa.ferry {
            cost += if self.profile.walking { WALKING_FERRY_ENTRY } else { FERRY_ENTRY };
        }
        if !self.profile.walking && !pushing(a) && mode == PUSH {
            cost += DISMOUNT_ENTRY;
        }
        Some(cost.round() as u64)
    }
}

fn cycle_routes(graph: &Graph) -> HashMap<i64, [u8; 2]> {
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
    let mut ways = HashMap::<i64, [u8; 2]>::new();
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
        for (member, role) in &relation.members {
            match member {
                Id::Way(id) => {
                    // Way roles refer to OSM node order, including inside a child relation.
                    let ranks = ways.entry(*id).or_default();
                    for (reversed, value) in ranks.iter_mut().enumerate() {
                        if role != if reversed == 0 { "backward" } else { "forward" } {
                            *value = (*value).max(rank);
                        }
                    }
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

    #[test]
    fn cycling_networks_favour_touring_by_level_without_discounting_walks_or_pushing() {
        use crate::source::{Relation, Way};
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
            Costing::new(graph, profile, &Doubts::default()).unwrap().roads[0].as_ref().unwrap().total()
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
        graph.osm.relations.get_mut(&10).unwrap().tags.remove("state");
        for role in ["", "forward", "backward"] {
            graph.osm.relations.get_mut(&10).unwrap().members[0].1 = role.into();
            for reversed in [false, true] {
                graph.roads[0].reversed = reversed;
                let opposite = if reversed { "forward" } else { "backward" };
                assert_eq!(
                    cost(&graph, "touring"),
                    if role == opposite { ordinary } else { previous },
                    "nested route member {role:?}, reversed={reversed}"
                );
            }
        }
        graph.roads[0].reversed = false;
        graph.osm.relations.get_mut(&10).unwrap().members[0].1.clear();
        for name in ["road", "touring"] {
            let mut profile = profiles.iter().find(|p| p.name == name).unwrap().clone();
            profile.route_bonus = 0.199;
            graph.roads[0].class = 2;
            graph.osm.ways.get_mut(&1).unwrap().tags =
                [("highway".into(), "primary".into()), ("maxspeed".into(), "90".into())].into_iter().collect();
            let source = &graph.osm.ways[&1];
            let floor = match profile.weighting {
                Weighting::RoadBike(variant) => {
                    road_bike::way(&graph.roads[0], &source.tags, Country::default(), variant, false)
                }
                Weighting::Weighted { .. } => {
                    suitability::way(&graph.roads[0], &source.tags, Country::default(), &profile)
                }
            }
            .unwrap()
            .minimum_factor;
            let factor = |graph: &Graph, doubts: &Doubts| {
                Costing::new(graph, &profile, doubts).unwrap().bases[0].unwrap().factor
            };
            let signed = factor(&graph, &Doubts::default());
            assert!(signed >= floor, "{name}");
            assert_eq!(factor(&graph, &Doubts { roads: vec![BIKE], ..Doubts::default() }), 3.0 * signed);
            graph.roads[0].class = 1;
            graph.osm.ways.get_mut(&1).unwrap().tags = [("highway".into(), "residential".into())].into_iter().collect();
            assert!(
                signed > factor(&graph, &Doubts::default()),
                "signed main road must not undercut a quiet road: {name}"
            );
        }
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
            crate::source::Way {
                id: 1,
                nodes: vec![0, 1, 2],
                tags: [("highway".into(), "residential".into())].into_iter().collect(),
            },
        );
        for profile in Profile::presets().into_iter().filter(|p| ["road", "touring"].contains(&p.name.as_str())) {
            assert!(Costing::new(&graph, &profile, &Doubts::default()).unwrap().transition(0, 1).is_some());
            for edge in 0..2 {
                graph.roads[edge].access = FOOT | PUSH;
                assert!(Costing::new(&graph, &profile, &Doubts::default()).unwrap().transition(0, 1).is_none());
                graph.roads[edge].access = BIKE;
            }
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
            roads: vec![road(points[..2].to_vec()), Road { from: 1, to: 2, ..road(points[1..].to_vec()) }],
            points,
            ..Graph::default()
        };
        let tags = [("highway".into(), "residential".into())].into_iter().collect();
        graph.osm.ways.insert(1, crate::source::Way { id: 1, nodes: vec![0, 1, 2], tags });
        let closure = |kind| (BIKE, Closure { kind, condition: String::new() });
        assert_eq!(Doubts::modes(&[closure(Kind::Seasonal), closure(Kind::Conditional)]), 0);
        assert_eq!(Doubts::modes(&[closure(Kind::Discouraged)]), BIKE);
        for name in ["touring", "road"] {
            let profile = Profile::presets().into_iter().find(|p| p.name == name).unwrap();
            let none = Doubts::default();
            let open = Costing::new(&graph, &profile, &none).unwrap();
            let doubts = Doubts { roads: vec![BIKE, FOOT | PUSH], nodes: [(1, BIKE)].into_iter().collect() };
            let doubted = Costing::new(&graph, &profile, &doubts).unwrap();
            let total = |costing: &Costing, road: usize| costing.roads[road].as_ref().unwrap().total();
            assert!(total(&doubted, 0).abs_diff(3 * total(&open, 0)) <= 2, "{name}");
            assert_eq!(total(&doubted, 1), total(&open, 1), "{name}");
            assert_eq!(doubted.transition(0, 1), open.transition(0, 1).map(|cost| cost + 300), "{name}");
        }
    }

    #[test]
    fn node_dismounts_keep_their_warning_cost_and_entries_apply_to_all_profiles() {
        let points: Vec<_> = (0..4).map(|i| point(i * 1000, 0.0)).collect();
        let mut graph = Graph {
            node_ids: vec![0, 1, 2, 3],
            node_access: vec![BIKE | FOOT | PUSH; 4],
            roads: (0..3)
                .map(|i| Road {
                    from: i as u32,
                    to: i as u32 + 1,
                    way: i as i64,
                    access: BIKE | FOOT | PUSH,
                    ..road(points[i..i + 2].to_vec())
                })
                .collect(),
            points,
            ..Graph::default()
        };
        for id in 0..3 {
            graph.osm.ways.insert(
                id,
                crate::source::Way {
                    id,
                    nodes: vec![id, id + 1],
                    tags: [("highway".into(), "residential".into())].into_iter().collect(),
                },
            );
        }
        let transition = |graph: &Graph, profile: &Profile, doubts: &Doubts, a, b| {
            Costing::new(graph, profile, doubts).unwrap().transition(a, b)
        };
        let none = Doubts::default();
        let doubts = Doubts { nodes: [(1, FOOT | PUSH)].into_iter().collect(), ..Doubts::default() };
        for mut profile in Profile::presets().into_iter().filter(|p| !p.name.contains('/')) {
            graph.node_access[1] = FOOT | PUSH;
            let entry = if profile.walking { 0 } else { 100 };
            assert_eq!(transition(&graph, &profile, &none, 0, 1), Some(entry));
            assert_eq!(transition(&graph, &profile, &doubts, 0, 1), Some(entry + 300));
            if !profile.walking {
                profile.allow_pushing = false;
                assert_eq!(transition(&graph, &profile, &none, 0, 1), None);
                profile.allow_pushing = true;
            }
            graph.node_access[1] = BIKE | FOOT | PUSH;
            for id in 1..3 {
                graph.roads[id].class = 6;
                graph.osm.ways.get_mut(&(id as i64)).unwrap().tags.insert("route".into(), "ferry".into());
            }
            assert_eq!(transition(&graph, &profile, &none, 0, 1), Some(if profile.walking { 100 } else { 1000 }));
            assert_eq!(transition(&graph, &profile, &none, 1, 2), Some(0));
            for id in 1..3 {
                graph.roads[id].class = 1;
                graph.osm.ways.get_mut(&(id as i64)).unwrap().tags.remove("route");
            }
        }
    }

    #[test]
    fn a_pushing_shortcut_must_save_more_than_a_trivial_cycling_detour() {
        let points = vec![point(0, 0.0), point(1000, 0.0), point(1100, 0.0), point(2100, 0.0)];
        let mut graph = Graph {
            node_ids: vec![0, 1, 2, 3],
            node_access: vec![BIKE | FOOT | PUSH; 4],
            roads: vec![
                Road { way: 0, ..road(points[..2].to_vec()) },
                Road { from: 1, to: 2, way: 1, class: 4, access: FOOT | PUSH, ..road(points[1..3].to_vec()) },
                Road { from: 2, to: 3, way: 2, ..road(points[2..].to_vec()) },
                Road { from: 1, to: 2, way: 3, ..road(vec![points[1], point(1050, 0.0), points[2]]) },
            ],
            points,
            ..Graph::default()
        };
        for id in 0..4 {
            let highway = if id == 1 { "footway" } else { "residential" };
            graph.osm.ways.insert(
                id,
                crate::source::Way {
                    id,
                    nodes: vec![],
                    tags: [("highway".into(), highway.into())].into_iter().collect(),
                },
            );
        }
        for lat in [200, 2000] {
            graph.roads[3].shape[1].lat = lat;
            graph.roads[3].length_m =
                graph.roads[3].shape.windows(2).map(|p| p[0].distance(p[1])).sum::<f64>().round() as u32;
            for profile in
                Profile::presets().into_iter().filter(|p| ["touring", "gravel", "mtb"].contains(&p.name.as_str()))
            {
                let doubts = Doubts::default();
                let costing = Costing::new(&graph, &profile, &doubts).unwrap();
                let path = |middle: usize| {
                    costing.roads[middle].as_ref().unwrap().total()
                        + costing.transition(0, middle as u32).unwrap()
                        + costing.transition(middle as u32, 2).unwrap()
                };
                assert_eq!(costing.transition(0, 1), Some(100));
                assert_eq!(path(1) < path(3), lat == 2000, "{}, {lat}", profile.name);
            }
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
