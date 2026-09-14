use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};

pub const BYTES: &[u8] = include_bytes!("policy.json");

#[derive(Deserialize)]
pub struct Policy {
    exclude_roots: BTreeSet<String>,
    groups: BTreeMap<String, Group>,
}
#[derive(Deserialize)]
struct Group {
    include: bool,
    roots: BTreeSet<String>,
}

/// Precedence is semantic, never a popularity or input-order ranking.
const CATEGORIES: [&str; 6] = [
    "Natural curiosities",
    "Castles and fortifications",
    "Archaeology and megaliths",
    "Monasteries and abbeys",
    "Cathedrals",
    "Passes",
];

impl Policy {
    pub fn load() -> Self {
        serde_json::from_slice(BYTES).expect("checked host category policy")
    }

    pub fn category(
        &self,
        instances: &[String],
        parents: &BTreeMap<String, Vec<String>>,
    ) -> Result<Option<u8>, &'static str> {
        let mut closure = BTreeSet::new();
        let mut remaining = instances.to_vec();
        let mut incomplete = false;
        while let Some(id) = remaining.pop() {
            if !closure.insert(id.clone()) {
                continue;
            }
            if closure.len() > 16384 {
                return Err("type_closure_budget");
            }
            if self.exclude_roots.contains(&id) {
                return Ok(None);
            }
            match parents.get(&id) {
                Some(ancestors) => remaining.extend(ancestors.iter().cloned()),
                None => incomplete = true,
            }
        }
        if incomplete {
            return Err("type_closure_missing");
        }
        Ok(CATEGORIES
            .iter()
            .position(|name| {
                let group = &self.groups[*name];
                group.include && !group.roots.is_disjoint(&closure)
            })
            .map(|index| index as u8 + 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn excluded_types_win_and_missing_closure_is_visible() {
        let policy = Policy::load();
        let parents = BTreeMap::from([("Q23413".into(), vec![]), ("Q133056".into(), vec![])]);
        assert_eq!(policy.category(&["Q23413".into()], &parents), Ok(Some(2)));
        assert_eq!(policy.category(&["Q133056".into()], &parents), Ok(Some(6)));
        assert_eq!(policy.category(&["Q23413".into(), "Q35666".into()], &parents), Ok(None));
        assert_eq!(policy.category(&["Q12345".into()], &parents), Err("type_closure_missing"));
    }
}
