use route_engine::cost::CostBasis;
use route_engine::{
    base,
    endpoints::Dictionary,
    model::{Profile, Road},
    package::Metric,
    table::Table,
};
use std::collections::BTreeMap;

/// A state is a directed road. Every profile shares all physically meeting road pairs.
pub(crate) fn edges(roads: &[Road]) -> Result<Vec<(u32, u32)>, String> {
    let mut outgoing = BTreeMap::<u32, Vec<u32>>::new();
    for (id, road) in roads.iter().enumerate() {
        outgoing.entry(road.from).or_default().push(u32::try_from(id).map_err(|_| "Too many roads")?);
    }
    let mut edges = Vec::new();
    for (before, road) in roads.iter().enumerate() {
        if let Some(after) = outgoing.get(&road.to) {
            edges.extend(after.iter().map(|&after| (before as u32, after)));
        }
    }
    u32::try_from(edges.len()).map_err(|_| "Too many road transitions")?;
    Ok(edges)
}

pub(crate) fn metric(
    profile: &Profile,
    bases: impl IntoIterator<Item = Option<CostBasis>>,
    road_costs: &[u64],
    turns: &[u64],
    dictionary: &mut Dictionary,
    mut write: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Metric, String> {
    let costs = bases
        .into_iter()
        .enumerate()
        .map(|(road, basis)| {
            if basis.is_some() != road_costs.get(road).is_some_and(|&cost| cost != u64::MAX) {
                return Err("Road cost and eligibility disagree".into());
            }
            basis.map(|basis| dictionary.insert(basis)).transpose().map(|id| id.unwrap_or(0))
        })
        .collect::<Result<Vec<_>, String>>()?;
    if costs.len() != road_costs.len() {
        return Err("Incomplete road costs".into());
    }
    let mut allowed = vec![0u64; costs.len().div_ceil(64)];
    for (id, &cost) in costs.iter().enumerate() {
        if cost != 0 {
            allowed[id / 64] |= 1 << (id % 64);
        }
    }
    Ok(Metric {
        profile: profile.clone(),
        weights: base::write_weights(road_costs, turns, &mut write)?,
        costs: Table::write(&costs, &mut write)?,
        allowed: Table::write(&allowed, &mut write)?,
    })
}
