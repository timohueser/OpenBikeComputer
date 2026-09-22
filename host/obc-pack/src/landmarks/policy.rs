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
const CATEGORIES: [&str; 16] = [
    "Natural curiosities",
    "Castles and fortifications",
    "Archaeology and megaliths",
    "Monasteries and abbeys",
    "Cathedrals",
    "Passes",
    "Mountain huts",
    "Observation towers",
    "Covered bridges, viaducts and aqueducts",
    "Dams",
    "Industrial heritage",
    "Pilgrimage churches and hermitages",
    "Lighthouses",
    "Boundary oddities",
    "Ghost towns",
    "Hot and mineral springs",
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
    fn category_ids_follow_the_policy_table() {
        let policy = Policy::load();
        let expected: [(&str, &[&str]); 16] = [
            (
                "Natural curiosities",
                &["Q34038", "Q150784", "Q35509", "Q954501", "Q1404150", "Q372934", "Q581776", "Q811534", "Q124714"],
            ),
            ("Castles and fortifications", &["Q23413", "Q17715832", "Q751876", "Q57831", "Q57821"]),
            ("Archaeology and megaliths", &["Q839954", "Q101659", "Q34023", "Q193475", "Q10521078", "Q53336805"]),
            ("Monasteries and abbeys", &["Q44613", "Q160742"]),
            ("Cathedrals", &["Q2977"]),
            ("Passes", &["Q133056"]),
            ("Mountain huts", &["Q182676", "Q339969"]),
            ("Observation towers", &["Q1440300"]),
            ("Covered bridges, viaducts and aqueducts", &["Q1825472", "Q181348", "Q12570", "Q474", "Q18870689"]),
            ("Dams", &["Q12323"]),
            (
                "Industrial heritage",
                &["Q820477", "Q1506469", "Q10832530", "Q59772", "Q185187", "Q38720", "Q40551", "Q1179791"],
            ),
            ("Pilgrimage churches and hermitages", &["Q10631691", "Q20064854", "Q56750657"]),
            ("Lighthouses", &["Q39715"]),
            ("Boundary oddities", &["Q316655", "Q921099", "Q590232", "Q55818"]),
            ("Ghost towns", &["Q74047", "Q350895"]),
            ("Hot and mineral springs", &["Q177380", "Q1365924"]),
        ];

        assert_eq!(policy.groups.len(), expected.len() + 1);
        assert!(!policy.groups["Glaciers"].include);
        assert_eq!(policy.groups["Glaciers"].roots, BTreeSet::from([String::from("Q35666")]));
        for (index, (name, roots)) in expected.iter().enumerate() {
            assert_eq!(CATEGORIES[index], *name);
            assert_eq!(policy.groups[*name].roots, roots.iter().copied().map(String::from).collect());
            for root in *roots {
                let parents = BTreeMap::from([(String::from(*root), vec![])]);
                assert_eq!(policy.category(&[String::from(*root)], &parents), Ok(Some(index as u8 + 1)));
            }
        }
    }

    #[test]
    fn exclusions_win_and_only_the_industrial_blanket_roots_are_removed() {
        let policy = Policy::load();
        let kept =
            ["Q131681", "Q3215290", "Q47486890", "Q96353874", "Q992794", "Q23397", "Q8502", "Q1015644", "Q35666"];
        assert_eq!(policy.exclude_roots, kept.into_iter().map(String::from).collect());

        for removed in ["Q1662011", "Q259209", "Q56284712"] {
            let parents = BTreeMap::from([
                (String::from("Q820477"), vec![String::from(removed)]),
                (String::from(removed), vec![]),
            ]);
            assert_eq!(policy.category(&[String::from("Q820477")], &parents), Ok(Some(11)));
        }

        let parents =
            BTreeMap::from([(String::from("Q182676"), vec![String::from("Q35666")]), (String::from("Q35666"), vec![])]);
        assert_eq!(policy.category(&[String::from("Q182676")], &parents), Ok(None));
        assert_eq!(policy.category(&["Q12345".into()], &parents), Err("type_closure_missing"));
    }
}
