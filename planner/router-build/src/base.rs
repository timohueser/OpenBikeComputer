use planner_router::cost::CostBasis;
use planner_router::{
    base,
    model::{Profile, Road},
    package::Metric,
    table::Table,
};
use std::collections::{BTreeMap, HashMap};

/// Interns exact cost parameters by their bits, without floating-point text conversion.
#[derive(Default)]
pub(crate) struct Dictionary {
    ids: HashMap<(u64, u64, bool), u32>,
    values: Vec<CostBasis>,
}

impl Dictionary {
    pub(crate) fn insert(&mut self, value: CostBasis) -> Result<u32, String> {
        if !value.valid() {
            return Err("Invalid cost basis".into());
        }
        let key = (value.factor.to_bits(), value.turn.to_bits(), value.ferry);
        if let Some(&id) = self.ids.get(&key) {
            return Ok(id);
        }
        let id = u32::try_from(self.values.len() + 1).map_err(|_| "Too many cost factors")?;
        self.ids.insert(key, id);
        self.values.push(value);
        Ok(id)
    }

    pub(crate) fn into_values(self) -> Vec<CostBasis> {
        self.values
    }
}

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

/// `allowed` holds the snap bits of `connectivity::States::snappable`.
pub(crate) fn metric(
    profile: &Profile,
    bases: impl IntoIterator<Item = Option<CostBasis>>,
    road_costs: &[u64],
    turns: &[u64],
    allowed: &[u64],
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
    if costs.len() != road_costs.len() || allowed.len() != costs.len().div_ceil(64) {
        return Err("Incomplete road costs".into());
    }
    if costs.iter().enumerate().any(|(id, &cost)| cost == 0 && allowed[id / 64] & (1 << (id % 64)) != 0) {
        return Err("A snappable road has no cost".into());
    }
    Ok(Metric {
        profile: profile.clone(),
        weights: base::write_weights(road_costs, turns, &mut write)?,
        costs: Table::write(&costs, &mut write)?,
        allowed: Table::write(allowed, &mut write)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dictionary_preserves_float_bits_and_rejects_invalid_costs() {
        let a = CostBasis { factor: f64::from_bits(0x3ff8a3d70a3d70a5), turn: 0.0, ferry: false };
        let b = CostBasis { turn: -0.0, ..a };
        let mut dictionary = Dictionary::default();
        assert_eq!(dictionary.insert(a).unwrap(), 1);
        assert_eq!(dictionary.insert(a).unwrap(), 1);
        assert_eq!(dictionary.insert(b).unwrap(), 2);
        assert!(dictionary.insert(CostBasis { factor: f64::NAN, ..a }).is_err());
        let values = dictionary.into_values();
        assert_eq!(values[0].factor.to_bits(), a.factor.to_bits());
        assert_eq!(values[1].turn.to_bits(), b.turn.to_bits());
    }
}
