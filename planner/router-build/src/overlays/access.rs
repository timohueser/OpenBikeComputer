//! Map access uses the importer's mode rules, without treating every one-way road as closed.
use crate::country::Country;
use crate::source::{self, Access, Way};
use planner_router::{
    closures::Kind,
    model::{BIKE, FOOT, PUSH},
};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn feature(way: &Way, country: Country) -> Option<Value> {
    let get = |key: &str| way.tags.get(key).map(String::as_str);
    if get("area") == Some("yes") {
        return None;
    }
    let construction = get("highway") == Some("construction");
    let defaults = if construction { 0 } else { source::highway_access(get("highway")?, country)?.1 };
    let conditional = source::conditional_modes(way.tags.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    let directions = ["forward", "backward"];
    // The router's modes; the strict modes also close every mode that an access value doubts.
    let access = |routing| directions.map(|d| if construction { 0 } else { source::access(get, defaults, d, routing) });
    let (modes, strict) = (access(true).map(|m| m & !conditional), access(false));
    let status = |walking| {
        if construction {
            return "construction";
        }
        let primary = if walking { FOOT } else { BIKE };
        if conditional & primary != 0 || !walking && modes.iter().any(|m| m & BIKE == 0) && conditional & PUSH != 0 {
            return "conditional";
        }
        if modes.iter().all(|m| m & primary != 0) {
            if strict.iter().all(|m| m & primary != 0) {
                return "";
            }
            let private = directions.iter().any(|&d| {
                source::inherited(&get, if walking { "foot" } else { "bicycle" }, d)
                    .is_some_and(|value| source::classify(value) == Access::Uncertain(Kind::Private))
            });
            return if private { "private" } else { "limited" };
        }
        if modes.iter().any(|m| m & primary != 0) {
            return "directional";
        }
        if !walking {
            if modes.iter().all(|m| m & PUSH != 0) {
                return "push";
            }
            if modes.iter().any(|m| m & PUSH != 0) {
                return "directional";
            }
            if modes.iter().any(|m| m & FOOT != 0) {
                return "no_bikes";
            }
        }
        "closed"
    };
    let (cycling, walking) = (status(false), status(true));
    if cycling.is_empty() && walking.is_empty() {
        return None;
    }
    let details: BTreeMap<_, _> = way
        .tags
        .iter()
        .filter(|(key, _)| {
            matches!(
                key.as_str(),
                "highway" | "construction" | "motorroad" | "opening_date" | "check_date" | "note" | "description"
            ) || matches!(key.split(':').next().unwrap_or(""), "access" | "vehicle" | "bicycle" | "foot")
                || key.ends_with(":conditional")
        })
        .collect();
    Some(json!({ "way": way.id, "kind": "access", "cycling_status": cycling, "walking_status": walking,
        "name": get("name").unwrap_or(""), "ref": get("ref").unwrap_or(""), "tags": details,
        "riding": modes.map(|m| m & BIKE != 0), "walking": modes.map(|m| m & FOOT != 0),
        "pushing": modes.map(|m| m & PUSH != 0), "conditional": conditional }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Tags;
    /// Tests use the worldwide defaults.
    fn feature(way: &Way) -> Option<Value> {
        super::feature(way, Country::default())
    }
    fn way(pairs: &[(&str, &str)]) -> Way {
        let mut tags: Tags = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        tags.entry("highway".into()).or_insert("path".into());
        Way { id: 1, nodes: vec![1, 2], tags }
    }
    #[test]
    fn restrictions_distinguish_pushing_private_access_and_construction() {
        for tags in [
            vec![("bicycle", "no")],
            vec![("bicycle", "dismount")],
            vec![("highway", "pedestrian")],
            vec![("highway", "cycleway"), ("bicycle", "dismount")],
        ] {
            let p = feature(&way(&tags)).unwrap();
            assert_eq!(p["cycling_status"], "push");
            assert_eq!(p["walking_status"], "");
            assert_eq!(p["pushing"], json!([true, true]));
        }
        for (tags, status) in [
            (vec![("bicycle", "no"), ("bicycle:pushing", "no")], "no_bikes"),
            (vec![("access", "no")], "closed"),
            (vec![("access", "private")], "private"),
            (vec![("access", "destination")], "limited"),
            (vec![("access", "permit")], "limited"),
            (vec![("access", "agricultural")], "limited"),
            (vec![("bicycle", "discouraged")], "limited"),
            (vec![("bicycle", "use_sidepath")], "limited"),
            (vec![("access", "no"), ("bicycle", "private")], "private"),
            (vec![("highway", "construction")], "construction"),
            (vec![("bicycle:forward", "no")], "directional"),
            (vec![("bicycle:conditional", "no @ (wet)")], "conditional"),
            (vec![("access:conditional", "no @ (Nov-May)")], "conditional"),
        ] {
            assert_eq!(feature(&way(&tags)).unwrap()["cycling_status"], status);
        }
        let conditional = feature(&way(&[("bicycle:conditional", "no @ (wet)")])).unwrap();
        assert_eq!(conditional["pushing"], json!([true, true]));
        assert_eq!(conditional["walking_status"], "");
    }
    #[test]
    fn restrictions_apply_to_the_relevant_modes_and_preserve_exceptions() {
        for tags in [
            vec![("access", "no"), ("bicycle", "yes"), ("foot", "yes")],
            vec![("access:conditional", "agricultural @ hazmat:water")],
            vec![("motor_vehicle:conditional", "no @ (wet)")],
            vec![("highway", ""), ("access", "private")],
            vec![("area", "yes"), ("access", "private")],
        ] {
            assert!(feature(&way(&tags)).is_none());
        }
        let p = feature(&way(&[("foot", "no")])).unwrap();
        assert_eq!(p["cycling_status"], "");
        assert_eq!(p["walking_status"], "closed");
        let p = feature(&way(&[("foot", "no"), ("bicycle", "no"), ("bicycle:pushing", "yes")])).unwrap();
        assert_eq!(p["cycling_status"], "closed");
        assert_eq!(p["pushing"], json!([false, false]));
    }
}
