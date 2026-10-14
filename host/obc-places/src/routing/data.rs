//! Source facts that must survive until all incidences of a junction are known.

use super::{ProfileAccess, RoutableWay};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberKind {
    Node,
    Way,
    Relation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TurnRestriction {
    pub from: i64,
    pub to: i64,
    pub via: i64,
    pub only: bool,
    pub u_turn: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Routing {
    pub nodes: BTreeMap<i64, ProfileAccess>,
    pub restrictions: Vec<TurnRestriction>,
    /// Unsupported turn rules close their approach ways instead of admitting an illegal turn.
    pub excluded_ways: BTreeSet<i64>,
    pub diagnostics: BTreeMap<&'static str, usize>,
}

impl Routing {
    pub fn count(&mut self, reason: &'static str) {
        *self.diagnostics.entry(reason).or_default() += 1;
    }

    /// Missing source members close the approach instead of dropping the restriction.
    pub fn validate(&mut self, ways: &[RoutableWay]) {
        let nodes: BTreeMap<_, _> = ways.iter().map(|way| (way.id, &way.node_ids)).collect();
        let mut incomplete = 0;
        let mut ambiguous = 0;
        for rule in &self.restrictions {
            if self.excluded_ways.contains(&rule.from) {
                continue;
            }
            if !nodes.get(&rule.from).is_some_and(|ids| ids.contains(&rule.via))
                || !nodes.get(&rule.to).is_some_and(|ids| ids.contains(&rule.via))
            {
                self.excluded_ways.insert(rule.from);
                incomplete += 1;
            } else {
                let incidences = |ids: &[i64]| {
                    ids.iter()
                        .enumerate()
                        .filter(|(_, id)| **id == rule.via)
                        .map(|(index, _)| usize::from(index > 0) + usize::from(index + 1 < ids.len()))
                        .sum::<usize>()
                };
                let unambiguous = if rule.from == rule.to {
                    incidences(nodes[&rule.from]) == 2
                } else {
                    incidences(nodes[&rule.from]) == 1 && incidences(nodes[&rule.to]) == 1
                };
                if !unambiguous {
                    self.excluded_ways.insert(rule.from);
                    ambiguous += 1;
                }
            }
        }
        if incomplete > 0 {
            *self.diagnostics.entry("incomplete turn rules: approach ways excluded").or_default() += incomplete;
        }
        if ambiguous > 0 {
            *self.diagnostics.entry("ambiguous turn rules: approach ways excluded").or_default() += ambiguous;
        }
    }

    pub fn relation<'a>(&mut self, tags: impl Fn(&str) -> Option<&'a str>, members: &[(i64, MemberKind, &'a str)]) {
        if !matches!(tags("type"), Some("restriction" | "restriction:bicycle" | "restriction:foot")) {
            return;
        }
        let except = tags("except").is_some_and(|value| value.split(';').any(|mode| mode.trim() == "bicycle"));
        let foot = tags("type") == Some("restriction:foot") || tags("restriction:foot").is_some();
        let conditional = tags("restriction:bicycle:conditional").is_some()
            || tags("restriction:foot:conditional").is_some()
            || (!except && tags("restriction:bicycle").is_none() && tags("restriction:conditional").is_some());
        let rule = tags("restriction:bicycle").or_else(|| (!except && !foot).then(|| tags("restriction")).flatten());
        if rule.is_none() && !foot && !conditional {
            return;
        }
        let from: Vec<_> =
            members.iter().filter(|(_, kind, role)| *kind == MemberKind::Way && *role == "from").map(|m| m.0).collect();
        let to: Vec<_> =
            members.iter().filter(|(_, kind, role)| *kind == MemberKind::Way && *role == "to").map(|m| m.0).collect();
        let via: Vec<_> = members.iter().filter(|(_, _, role)| *role == "via").collect();
        let supported = rule.is_some_and(|value| {
            matches!(
                value,
                "no_left_turn"
                    | "no_right_turn"
                    | "no_straight_on"
                    | "no_u_turn"
                    | "only_left_turn"
                    | "only_right_turn"
                    | "only_straight_on"
                    | "only_u_turn"
            )
        });
        if !foot
            && !conditional
            && supported
            && from.len() == 1
            && to.len() == 1
            && via.len() == 1
            && via[0].1 == MemberKind::Node
        {
            let rule = rule.unwrap();
            self.restrictions.push(TurnRestriction {
                from: from[0],
                to: to[0],
                via: via[0].0,
                only: rule.starts_with("only_"),
                u_turn: rule.ends_with("u_turn"),
            });
        } else {
            self.count("unsupported turn rules: approach ways excluded");
            self.excluded_ways.extend(if from.is_empty() {
                members.iter().filter(|(_, kind, _)| *kind == MemberKind::Way).map(|m| m.0).collect()
            } else {
                from
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn way(id: i64, nodes: Vec<i64>) -> RoutableWay {
        let coords = nodes.iter().map(|node| (*node as i32, 0)).collect();
        RoutableWay::new(id, nodes, coords, 7)
    }

    #[test]
    fn turn_members_require_a_node_and_supported_unconditional_shape() {
        let tags = |key: &str| match key {
            "type" => Some("restriction"),
            "restriction" => Some("only_right_turn"),
            _ => None,
        };
        for kind in [MemberKind::Way, MemberKind::Relation] {
            let mut routing = Routing::default();
            routing.relation(tags, &[(10, MemberKind::Way, "from"), (20, MemberKind::Way, "to"), (2, kind, "via")]);
            assert!(routing.restrictions.is_empty());
            assert!(routing.excluded_ways.contains(&10));
        }
        let mut routing = Routing::default();
        routing.relation(
            tags,
            &[(10, MemberKind::Way, "from"), (20, MemberKind::Way, "to"), (2, MemberKind::Node, "via")],
        );
        routing.validate(&[way(10, vec![1, 2]), way(20, vec![2, 3])]);
        assert_eq!(routing.restrictions.len(), 1);
        assert!(routing.excluded_ways.is_empty());
    }

    #[test]
    fn incomplete_or_ambiguous_turns_close_the_approach() {
        let rule = TurnRestriction { from: 10, to: 20, via: 2, only: true, u_turn: false };
        for ways in [vec![way(10, vec![1, 2])], vec![way(10, vec![1, 2]), way(20, vec![3, 2, 4])]] {
            let mut routing = Routing { restrictions: vec![rule], ..Routing::default() };
            routing.validate(&ways);
            assert!(routing.excluded_ways.contains(&10));
            assert_eq!(routing.diagnostics.values().sum::<usize>(), 1);
        }
    }
}
