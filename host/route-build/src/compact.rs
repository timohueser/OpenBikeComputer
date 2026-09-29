use crate::cost::Costing;
use route_engine::{
    model::{Graph, Profile},
    package::{Departure, Endpoint},
};
use std::collections::BTreeMap;

pub struct Arc {
    pub to: u32,
    pub road: u32,
    pub cost: u64,
}
pub struct Compact {
    pub arcs: Vec<Vec<Arc>>,
    pub endpoints: Vec<Endpoint>,
}

impl Compact {
    pub fn new(graph: &Graph, profile: &Profile) -> Result<Self, String> {
        let costing = Costing::new(graph, profile)?;
        let mut endpoints: Vec<_> =
            costing.roads.iter().map(|cost| Endpoint { cost: cost.clone(), ..Endpoint::default() }).collect();
        let mut incoming = vec![Vec::new(); graph.points.len()];
        let mut outgoing = vec![Vec::new(); graph.points.len()];
        for (id, road) in graph.roads.iter().enumerate() {
            if road.from as usize >= graph.points.len() || road.to as usize >= graph.points.len() {
                return Err("Invalid road endpoints".into());
            }
            if endpoints[id].cost.is_some() {
                incoming[road.to as usize].push(id as u32);
                outgoing[road.from as usize].push(id as u32);
            }
        }
        let restrictions = if profile.walking { &graph.forbidden_foot } else { &graph.forbidden };
        if !restrictions.is_sorted() {
            return Err("Unsorted turn restrictions".into());
        }
        for &(a, b) in restrictions {
            let from = graph.roads.get(a as usize).ok_or("Unknown restriction road")?;
            let to = graph.roads.get(b as usize).ok_or("Unknown restriction road")?;
            if from.to != to.from {
                return Err("Restriction roads do not meet".into());
            }
        }
        let mut arcs = Vec::<Vec<Arc>>::new();
        for node in 0..graph.points.len() {
            // Arrivals share a state only when every legal departure and its penalty agree.
            let mut groups = BTreeMap::new();
            for &before in &incoming[node] {
                let penalties: Vec<_> = outgoing[node].iter().map(|&after| costing.transition(before, after)).collect();
                let state = if let Some(&state) = groups.get(&penalties) {
                    state
                } else {
                    let state = u32::try_from(arcs.len()).map_err(|_| "Too many states")?;
                    for (&after, &penalty) in outgoing[node].iter().zip(&penalties) {
                        if let Some(penalty) = penalty {
                            endpoints[after as usize].departures.push(Departure { state, penalty });
                        }
                    }
                    groups.insert(penalties, state);
                    arcs.push(Vec::new());
                    state
                };
                endpoints[before as usize].arrival = state;
            }
        }
        for (road, endpoint) in endpoints.iter().enumerate() {
            if let Some(cost) = &endpoint.cost {
                for departure in &endpoint.departures {
                    arcs[departure.state as usize].push(Arc {
                        to: endpoint.arrival,
                        road: road as u32,
                        cost: cost.total().checked_add(departure.penalty).ok_or("Transition cost overflow")?,
                    });
                }
            }
        }
        Ok(Self { arcs, endpoints })
    }
}
