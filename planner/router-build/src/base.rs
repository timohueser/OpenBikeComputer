use crate::{column::Column, Roads};
use planner_router::cost::CostBasis;
use planner_router::{base, model::Profile, package::Metric, table::Table};
use std::collections::HashMap;

/// Interns exact cost parameters by their bits, without floating-point text conversion.
#[derive(Default)]
pub struct Dictionary {
    ids: HashMap<(u64, u64, bool), u32>,
    values: Vec<CostBasis>,
}

impl Dictionary {
    pub(crate) fn insert(&mut self, value: CostBasis) -> Result<u32, String> {
        if !value.valid() {
            return Err("Invalid cost basis".into());
        }
        self.intern(value)
    }

    /// `insert` without the check of the value.
    pub(crate) fn intern(&mut self, value: CostBasis) -> Result<u32, String> {
        let key = (value.factor.to_bits(), value.turn.to_bits(), value.ferry);
        if let Some(&id) = self.ids.get(&key) {
            return Ok(id);
        }
        let id = u32::try_from(self.values.len() + 1).map_err(|_| "Too many cost factors")?;
        self.ids.insert(key, id);
        self.values.push(value);
        Ok(id)
    }

    /// The basis of `id`; `id` 0 is no basis.
    pub(crate) fn get(&self, id: u32) -> Option<CostBasis> {
        Some(self.values[id.checked_sub(1)? as usize])
    }

    pub(crate) fn into_values(self) -> Vec<CostBasis> {
        self.values
    }
}

/// The road-state graph that every profile shares: a state is a directed road, and road `r` leads
/// to `head[arc]` for each arc in `first[r]..first[r + 1]`. Those are the roads that depart where
/// `r` arrives, in road order.
pub struct Topology {
    pub first: Column<u32>,
    pub head: Column<u32>,
}

impl Topology {
    /// `roads` fits u32, and each road ends at one of `points`.
    pub fn new(roads: &Roads, points: usize) -> Result<Self, String> {
        let (start, departures) = group(roads.from.values(), points);
        let (start, departures) = (start.values(), departures.values());
        let mut first = Column::default();
        let mut head = Column::default();
        first.push(0);
        for before in 0..roads.len() {
            let to = roads.to(before) as usize;
            head.extend(&departures[start[to] as usize..start[to + 1] as usize]);
            first.push(u32::try_from(head.values().len()).map_err(|_| "Too many road transitions")?);
        }
        Ok(Self { first, head })
    }

    pub fn roads(&self) -> usize {
        self.first.values().len() - 1
    }

    /// Each arc as (road, next road), in arc order.
    pub fn edges(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        let (first, head) = (self.first.values(), self.head.values());
        (0..self.roads()).flat_map(move |road| {
            head[first[road] as usize..first[road + 1] as usize].iter().map(move |&next| (road as u32, next))
        })
    }
}

/// The indices of `keys` grouped by key, in index order: group `g` is
/// `members[start[g]..start[g + 1]]`. Each key is below `groups`, and `keys` fits u32.
pub(crate) fn group(keys: &[u32], groups: usize) -> (Column<u32>, Column<u32>) {
    let mut start = Column::filled(groups + 1, 0u32);
    let counts = start.values_mut();
    for &key in keys {
        counts[key as usize + 1] += 1;
    }
    for group in 0..groups {
        counts[group + 1] += counts[group];
    }
    let mut cursor: Column<u32> = counts[..groups].iter().copied().collect();
    let cursor = cursor.values_mut();
    let mut members = Column::filled(keys.len(), 0u32);
    for (index, &key) in keys.iter().enumerate() {
        members.values_mut()[cursor[key as usize] as usize] = index as u32;
        cursor[key as usize] += 1;
    }
    (start, members)
}

/// A cost in a 32-bit working column, where `u32::MAX` is unavailable as in the stored columns.
pub(crate) fn narrow(cost: Option<u64>) -> Result<u32, String> {
    match cost {
        None => Ok(u32::MAX),
        Some(cost) => u32::try_from(cost).ok().filter(|&cost| cost != u32::MAX).ok_or("A cost exceeds 32 bits".into()),
    }
}

fn widen(costs: &[u32]) -> impl Iterator<Item = u64> + Clone + '_ {
    costs.iter().map(|&cost| if cost == u32::MAX { u64::MAX } else { cost as u64 })
}

/// `bases` holds the dictionary index of each road, and `allowed` the snap bits of
/// `connectivity::States::snappable`.
pub(crate) fn metric(
    profile: &Profile,
    bases: &[u32],
    road_costs: &[u32],
    turns: &[u32],
    allowed: &[u64],
    mut write: impl FnMut(&[u8]) -> Result<String, String>,
) -> Result<Metric, String> {
    if bases.len() != road_costs.len() || allowed.len() != bases.len().div_ceil(64) {
        return Err("Incomplete road costs".into());
    }
    if bases.iter().zip(road_costs).any(|(&basis, &cost)| (basis != 0) != (cost != u32::MAX)) {
        return Err("Road cost and eligibility disagree".into());
    }
    if bases.iter().enumerate().any(|(id, &basis)| basis == 0 && allowed[id / 64] & (1 << (id % 64)) != 0) {
        return Err("A snappable road has no cost".into());
    }
    Ok(Metric {
        profile: profile.clone(),
        weights: base::write_weights(widen(road_costs), widen(turns), &mut write)?,
        costs: Table::write(bases, &mut write)?,
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
