use route_engine::{
    model::{Graph, Profile},
    package::Endpoint,
};

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
        profile.validate()?;
        let mut endpoints: Vec<_> =
            graph.roads.iter().map(|r| Endpoint { cost: profile.cost(r), ..Endpoint::default() }).collect();
        let mut incoming = vec![Vec::new(); graph.points.len()];
        let mut outgoing = vec![Vec::new(); graph.points.len()];
        for (id, road) in graph.roads.iter().enumerate() {
            if road.from as usize >= graph.points.len() || road.to as usize >= graph.points.len() {
                return Err("Invalid road endpoints".into());
            }
            if endpoints[id].cost.is_some() {
                incoming[road.to as usize].push(id);
                outgoing[road.from as usize].push(id);
            }
        }
        let restrictions = if profile.walking { &graph.forbidden_foot } else { &graph.forbidden };
        if !restrictions.is_sorted() {
            return Err("Unsorted turn restrictions".into());
        }
        let mut restricted = vec![false; graph.points.len()];
        for &(a, b) in restrictions {
            let from = graph.roads.get(a as usize).ok_or("Unknown restriction road")?;
            let to = graph.roads.get(b as usize).ok_or("Unknown restriction road")?;
            if from.to != to.from {
                return Err("Restriction roads do not meet".into());
            }
            if endpoints[a as usize].cost.is_some() && endpoints[b as usize].cost.is_some() {
                restricted[from.to as usize] = true;
            }
        }
        let mut arcs = Vec::<Vec<Arc>>::new();
        for node in 0..graph.points.len() {
            if incoming[node].is_empty() && outgoing[node].is_empty() {
                continue;
            }
            let first = u32::try_from(arcs.len()).map_err(|_| "Too many states")?;
            if restricted[node] {
                for &road in &incoming[node] {
                    endpoints[road].arrival = u32::try_from(arcs.len()).map_err(|_| "Too many states")?;
                    arcs.push(Vec::new());
                }
            } else {
                arcs.push(Vec::new());
                for &road in &incoming[node] {
                    endpoints[road].arrival = first;
                }
            }
            for &road in &outgoing[node] {
                endpoints[road].departures = if restricted[node] {
                    incoming[node]
                        .iter()
                        .filter(|&&before| graph.permits_turn(before as u32, road as u32, profile.walking))
                        .map(|&before| endpoints[before].arrival)
                        .collect()
                } else {
                    vec![first]
                };
            }
        }
        for (road, endpoint) in endpoints.iter().enumerate() {
            if let Some(cost) = endpoint.cost {
                for &from in &endpoint.departures {
                    arcs[from as usize].push(Arc { to: endpoint.arrival, road: road as u32, cost });
                }
            }
        }
        Ok(Self { arcs, endpoints })
    }
}
