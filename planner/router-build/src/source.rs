//! The source OSM model and the tag rules that the import, the overlay index and the route
//! catalog share. No source object enters the routing package.
use crate::country::Country;
use planner_router::closures::{Closure, Kind};
use planner_router::model::{Point, BIKE, FOOT, PUSH};
use std::collections::BTreeMap;

pub type Tags = BTreeMap<String, String>;

/// What an access value tells the router. Only `no` blocks a mode; every other restriction stays
/// routable, and the route reports it as a closure.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Open,
    Uncertain(Kind),
    Closed,
}

/// A list such as `agricultural;forestry` takes its most open value.
pub fn classify(value: &str) -> Access {
    let one = |value: &str| match value.trim() {
        // `mtb` designates a way for mountain bikes, as in Switzerland.
        "yes" | "designated" | "official" | "permissive" | "mtb" | "optional_sidepath" => Access::Open,
        // `dismount` closes riding only; pushing follows foot access.
        "no" | "dismount" => Access::Closed,
        "permit" => Access::Uncertain(Kind::Permit),
        "private" => Access::Uncertain(Kind::Private),
        "agricultural" | "forestry" => Access::Uncertain(Kind::Farm),
        "use_sidepath" => Access::Uncertain(Kind::Sidepath),
        "discouraged" => Access::Uncertain(Kind::Discouraged),
        "destination" | "customers" | "delivery" | "residents" | "military" | "psv" | "bus" | "emergency" | "hgv"
        | "taxi" | "motor_vehicle" | "motorcar" => Access::Uncertain(Kind::Limited),
        _ => Access::Uncertain(Kind::Unclear),
    };
    value.split(';').map(one).min().unwrap_or(Access::Closed)
}

pub fn granted(value: &str) -> bool {
    classify(value) == Access::Open
}

/// Road class and default access, before explicit tags and one-way rules. The modes are the
/// worldwide defaults, unless the way's country differs.
pub fn highway_access(highway: &str, country: Country) -> Option<(u8, u8)> {
    let (class, modes) = match highway {
        "cycleway" => (0, BIKE),
        "residential" | "living_street" | "unclassified" | "service" | "tertiary" | "tertiary_link" => {
            (1, BIKE | FOOT | PUSH)
        }
        "trunk" | "trunk_link" | "primary" | "primary_link" | "secondary" | "secondary_link" => (2, BIKE | FOOT | PUSH),
        "motorway" | "motorway_link" => (2, 0),
        "track" | "road" => (3, BIKE | FOOT | PUSH),
        "path" => (4, BIKE | FOOT | PUSH),
        "footway" | "pedestrian" => (4, FOOT | PUSH),
        "bridleway" => (4, 0),
        "steps" => (5, FOOT | PUSH),
        _ => return None,
    };
    Some((class, country.defaults(highway).unwrap_or(modes)))
}

pub fn inherited<'a>(get: &impl Fn(&str) -> Option<&'a str>, mode: &str, direction: &str) -> Option<&'a str> {
    let keys: &[&str] = if mode == "bicycle" { &["access", "vehicle", "bicycle"] } else { &["access", "foot"] };
    let mut result = None;
    for key in keys {
        result = get(key).or(result);
        result = get(&format!("{key}:{direction}")).or(result);
    }
    result
}

/// The direction that a one-way rule leaves open for a mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Oneway {
    Both,
    Forward,
    Backward,
    /// The direction changes over the day; the route reports it as a closure.
    Reversible,
    Unknown,
}

fn oneway_value(value: &str) -> Oneway {
    match value {
        "yes" | "1" | "true" => Oneway::Forward,
        "-1" | "reverse" => Oneway::Backward,
        // Traffic takes turns, as on a one-lane bridge with lights.
        "no" | "0" | "false" | "alternating" => Oneway::Both,
        "reversible" => Oneway::Reversible,
        _ => Oneway::Unknown,
    }
}

/// The road's own one-way rule; a roundabout implies one.
fn road_oneway<'a>(get: &impl Fn(&str) -> Option<&'a str>) -> Oneway {
    let implied = get("junction") == Some("roundabout");
    oneway_value(get("oneway").unwrap_or(if implied { "yes" } else { "no" }))
}

/// The one-way rule for `mode` ("bicycle" or "foot"). `oneway` binds riders only, and not where a
/// cycle lane runs against it.
pub fn oneway<'a>(get: impl Fn(&str) -> Option<&'a str>, mode: &str) -> Oneway {
    if mode == "foot" {
        return get("oneway:foot").map_or(Oneway::Both, oneway_value);
    }
    if let Some(value) = get("oneway:bicycle") {
        return oneway_value(value);
    }
    match road_oneway(&get) {
        // A lane on a one-way road follows the road, so the side of traffic does not matter.
        Oneway::Forward if cycleway(&get, true, false) => Oneway::Both,
        Oneway::Backward if cycleway(&get, false, false) => Oneway::Both,
        oneway => oneway,
    }
}

/// Whether a cycle lane runs in the travel direction. A lane on a two-way road without a direction
/// runs with the traffic on its side of the road.
pub fn cycleway<'a>(get: impl Fn(&str) -> Option<&'a str>, reversed: bool, left_hand: bool) -> bool {
    let road_direction = match road_oneway(&get) {
        Oneway::Forward => Some(false),
        Oneway::Backward => Some(true),
        _ => None,
    };
    ["cycleway", "cycleway:left", "cycleway:right"].iter().any(|key| {
        let both = *key != "cycleway";
        let value = get(key).or_else(|| both.then(|| get("cycleway:both")).flatten()).unwrap_or("");
        if !matches!(
            value,
            "lane"
                | "track"
                | "shared_lane"
                | "share_busway"
                | "shoulder"
                | "opposite"
                | "opposite_lane"
                | "opposite_track"
        ) {
            return false;
        }
        match get(&format!("{key}:oneway")).or_else(|| both.then(|| get("cycleway:both:oneway")).flatten()) {
            Some("no" | "0" | "false") => true,
            Some("yes" | "1" | "true") => !reversed,
            Some("-1" | "reverse") => reversed,
            Some(_) => false,
            None if value.starts_with("opposite") => reversed != road_direction.unwrap_or(false),
            None => road_direction.map_or(
                match *key {
                    "cycleway:left" => reversed != left_hand,
                    "cycleway:right" => reversed == left_hand,
                    _ => true,
                },
                |direction| reversed == direction,
            ),
        }
    })
}

/// Pushing follows pedestrian access, unless explicitly restricted.
/// With `routing`, an uncertain value opens its mode, and the route reports it as a closure.
pub fn access<'a>(get: impl Fn(&str) -> Option<&'a str>, defaults: u8, direction: &str, routing: bool) -> u8 {
    let opens = |value: &str| match classify(value) {
        Access::Open => true,
        Access::Uncertain(_) => routing,
        Access::Closed => false,
    };
    let restricted_road =
        get("motorroad") == Some("yes") || matches!(get("highway"), Some("motorway" | "motorway_link"));
    let mut result = if restricted_road { 0 } else { defaults };
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        if let Some(value) = inherited(&get, mode, direction) {
            if opens(value) {
                result |= bits;
            } else {
                result &= !bits;
            }
        }
        if restricted_road && !get(&format!("{mode}:{direction}")).or(get(mode)).is_some_and(granted) {
            result &= !bits;
        }
    }
    if !restricted_road
        && inherited(&get, "bicycle", direction) == Some("dismount")
        && get(&format!("foot:{direction}")).or(get("foot")).is_none_or(granted)
    {
        result |= FOOT | PUSH;
    }
    if !restricted_road && inherited(&get, "bicycle", direction).is_none() && get("bicycle_road") == Some("yes") {
        result |= BIKE;
    }
    if let Some(value) = get(&format!("bicycle:pushing:{direction}")).or(get("bicycle:pushing")) {
        if opens(value) {
            result |= PUSH;
        } else {
            result &= !PUSH;
        }
    }
    if result & FOOT == 0 {
        result &= !PUSH;
    }
    result
}

/// Modes with a conditional restriction. The router keeps them open; the map shows them.
pub fn conditional_modes<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>) -> u8 {
    conditions(tags).filter(|(_, _, clauses)| !clauses.is_empty()).fold(0, |modes, (_, m, _)| modes | m)
}

/// What may close a road that the router keeps open: an uncertain access value, a reversible
/// one-way, or a conditional restriction. Only a restricting clause counts: `yes @ (May-Oct)`
/// names the open season. A conditional one-way closes one direction only, so it is no closure of
/// the road. Only the tags of the given `directions` of travel count.
pub fn closures<'a>(tags: impl Iterator<Item = (&'a str, &'a str)>, directions: &[&str]) -> Vec<(u8, Closure)> {
    let tags: BTreeMap<&str, &str> = tags.collect();
    let get = |key: &str| tags.get(key).copied();
    let mut found = Vec::new();
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        for &direction in directions {
            if let Some(value) = inherited(&get, mode, direction) {
                if let Access::Uncertain(kind) = classify(value) {
                    found.push((bits, kind, value));
                }
            }
        }
    }
    for (mode, bits) in [("foot", FOOT | PUSH), ("bicycle", BIKE)] {
        if oneway(get, mode) == Oneway::Reversible {
            found.push((bits, Kind::Unclear, "oneway=reversible"));
        }
    }
    for (key, modes, clauses) in conditions(tags.iter().map(|(key, value)| (*key, *value))) {
        let other = ["forward", "backward"].into_iter().find(|d| key.ends_with(d) && !directions.contains(d));
        if modes == 0 || key.starts_with("oneway") || other.is_some() {
            continue;
        }
        for (_, condition) in clauses.into_iter().filter(|(value, _)| !granted(value)) {
            found.push((modes, if seasonal(condition) { Kind::Seasonal } else { Kind::Conditional }, condition));
        }
    }
    let mut closures: Vec<(u8, Closure)> = Vec::new();
    for (modes, kind, condition) in found {
        match closures.iter_mut().find(|(_, known)| known.kind == kind && known.condition == condition) {
            Some((known, _)) => *known |= modes,
            None => closures.push((modes, Closure { kind, condition: condition.to_owned() })),
        }
    }
    closures
}

/// The key, modes and `(value, condition)` clauses of each conditional tag. A hazardous-load
/// clause never concerns a rider or walker, so it is left out.
fn conditions<'a, I: Iterator<Item = (&'a str, &'a str)>>(
    tags: I,
) -> impl Iterator<Item = (&'a str, u8, Vec<(&'a str, &'a str)>)> + use<'a, I> {
    tags.filter_map(|(key, value)| {
        let key = key.strip_suffix(":conditional")?;
        let modes = match key {
            "access" | "access:forward" | "access:backward" => BIKE | FOOT | PUSH,
            "vehicle" | "vehicle:forward" | "vehicle:backward" | "bicycle" | "bicycle:forward" | "bicycle:backward"
            | "oneway" | "oneway:bicycle" => BIKE,
            "bicycle:pushing" | "bicycle:pushing:forward" | "bicycle:pushing:backward" => PUSH,
            "foot" | "foot:forward" | "foot:backward" | "oneway:foot" => FOOT | PUSH,
            _ => 0,
        };
        let clauses = value
            .split(';')
            .map(|clause| {
                let (value, condition) = clause.split_once('@').unwrap_or((clause, ""));
                (value.trim(), condition.trim().trim_matches(['(', ')']).trim())
            })
            .filter(|(_, condition)| !hazmat(condition))
            .collect();
        Some((key, modes, clauses))
    })
}

/// A hazardous-load condition never concerns a rider or walker.
fn hazmat(condition: &str) -> bool {
    matches!(condition, "hazmat" | "hazmat:water")
}

/// A closure such as `Nov-May` moves with the snow each year; the snow layer tells the rider
/// when the road is usually clear. A dated closure with a year is not seasonal.
fn seasonal(condition: &str) -> bool {
    const MONTHS: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    const SEASONS: [&str; 4] = ["winter", "spring", "summer", "autumn"];
    let condition = condition.to_ascii_lowercase();
    let mut named = false;
    let dates = condition.split([' ', '-', ',']).filter(|token| !token.is_empty()).all(|token| {
        let day = token.trim_end_matches(char::is_alphabetic);
        if !day.is_empty() {
            let suffix = &token[day.len()..];
            return matches!(suffix, "" | "st" | "nd" | "rd" | "th")
                && day.parse::<u8>().is_ok_and(|d| (1..=31).contains(&d));
        }
        let name = SEASONS.contains(&token) || MONTHS.iter().any(|m| token.starts_with(m));
        named |= name;
        name
    });
    dates && named
}

#[derive(Clone, Debug)]
pub struct Node {
    pub id: i64,
    pub point: Point,
    pub tags: Tags,
}

#[derive(Clone, Debug)]
pub struct Way {
    pub id: i64,
    pub nodes: Vec<i64>,
    pub tags: Tags,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Id {
    Node(i64),
    Way(i64),
    Relation(i64),
}

#[derive(Clone, Debug)]
pub struct Relation {
    pub id: i64,
    pub tags: Tags,
    pub members: Vec<(Id, String)>,
}

#[derive(Clone, Debug, Default)]
pub struct Data {
    pub nodes: BTreeMap<i64, Node>,
    pub ways: BTreeMap<i64, Way>,
    pub relations: BTreeMap<i64, Relation>,
}

impl Data {
    pub fn country(&self, way: &Way) -> Country {
        Country::of_way(&way.nodes, |id| self.nodes.get(&id).map(|node| node.point))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oneway_binds_riders_unless_a_lane_or_tag_frees_them() {
        let rule =
            |tags: &[(&str, &str)], mode| oneway(|key| tags.iter().find(|(k, _)| *k == key).map(|(_, v)| *v), mode);
        assert_eq!(rule(&[("oneway", "yes")], "bicycle"), Oneway::Forward);
        assert_eq!(rule(&[("oneway", "yes")], "foot"), Oneway::Both);
        assert_eq!(rule(&[("junction", "roundabout")], "bicycle"), Oneway::Forward);
        assert_eq!(rule(&[("junction", "roundabout"), ("oneway", "no")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("junction", "circular")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("junction", "circular"), ("oneway", "yes")], "bicycle"), Oneway::Forward);
        assert_eq!(rule(&[("junction", "circular"), ("oneway", "no")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "alternating")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "-1"), ("oneway:bicycle", "no")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "yes"), ("cycleway:left", "opposite_lane")], "bicycle"), Oneway::Both);
        assert_eq!(rule(&[("oneway", "yes"), ("cycleway:left", "lane")], "bicycle"), Oneway::Forward);
        let reversible = closures([("oneway", "reversible")].into_iter(), &["forward"]);
        assert_eq!(reversible, vec![(BIKE, Closure { kind: Kind::Unclear, condition: "oneway=reversible".into() })]);
    }

    #[test]
    fn a_lane_without_a_direction_runs_with_the_traffic_on_its_side() {
        let get = |key: &str| (key == "cycleway:left").then_some("lane");
        assert_eq!([false, true].map(|reversed| cycleway(get, reversed, false)), [false, true]);
        assert_eq!([false, true].map(|reversed| cycleway(get, reversed, true)), [true, false]);
    }

    #[test]
    fn only_no_closes_and_every_other_restriction_becomes_a_closure() {
        use Access::{Closed, Open, Uncertain};
        for (value, expected) in [
            ("yes;designated", Open),
            ("mtb", Open),
            ("optional_sidepath", Open),
            ("no", Closed),
            ("dismount", Closed),
            ("permit", Uncertain(Kind::Permit)),
            ("private", Uncertain(Kind::Private)),
            ("agricultural;forestry", Uncertain(Kind::Farm)),
            ("use_sidepath", Uncertain(Kind::Sidepath)),
            ("discouraged", Uncertain(Kind::Discouraged)),
            ("customers", Uncertain(Kind::Limited)),
            ("motor_vehicle;emergency", Uncertain(Kind::Limited)),
            ("military", Uncertain(Kind::Limited)),
            ("no;private", Uncertain(Kind::Private)),
            ("service", Uncertain(Kind::Unclear)),
        ] {
            assert_eq!(classify(value), expected, "{value}");
        }
        let permit = |key: &str| (key == "access").then_some("permit");
        assert_eq!(access(permit, BIKE | FOOT | PUSH, "forward", true), BIKE | FOOT | PUSH);
        assert_eq!(access(permit, BIKE | FOOT | PUSH, "forward", false), 0);

        let closures = |tags: &[(&'static str, &'static str)]| closures(tags.iter().copied(), &["forward", "backward"]);
        let closure = |kind, condition: &str| Closure { kind, condition: condition.into() };
        let all = BIKE | FOOT | PUSH;
        assert_eq!(
            closures(&[
                ("access", "permit"),
                ("vehicle", "permit"),
                ("access:conditional", "no @ Oct 14th - May 31st")
            ]),
            vec![(all, closure(Kind::Permit, "permit")), (all, closure(Kind::Seasonal, "Oct 14th - May 31st"))]
        );
        assert_eq!(
            closures(&[("access", "destination"), ("bicycle", "yes"), ("bicycle:conditional", "no @ (wet)")]),
            vec![(FOOT | PUSH, closure(Kind::Limited, "destination")), (BIKE, closure(Kind::Conditional, "wet"))]
        );
        assert_eq!(
            closures(&[("access", "private"), ("bicycle", "use_sidepath")]),
            vec![(FOOT | PUSH, closure(Kind::Private, "private")), (BIKE, closure(Kind::Sidepath, "use_sidepath"))]
        );
        let one_way = [("bicycle:backward", "use_sidepath"), ("bicycle:backward:conditional", "no @ (wet)")];
        assert_eq!(super::closures(one_way.into_iter(), &["forward"]), vec![]);
        assert_eq!(super::closures(one_way.into_iter(), &["backward"]).len(), 2);
        assert_eq!(
            closures(&[
                ("access:conditional", "no @ (Nov-May)"),
                ("foot:conditional", "no @ (2026 Mar 1-2026 Jul 31)")
            ]),
            vec![
                (all, closure(Kind::Seasonal, "Nov-May")),
                (FOOT | PUSH, closure(Kind::Conditional, "2026 Mar 1-2026 Jul 31"))
            ]
        );
        // An open season, a one-way, a motor-vehicle rule and a freight rule close nothing.
        for tag in [
            ("access:conditional", "yes @ (May-Oct)"),
            ("oneway:conditional", "yes @ (Mo-Fr 07:00-09:00)"),
            ("motor_vehicle:conditional", "no @ (wet)"),
            ("access:conditional", "agricultural @ hazmat:water"),
        ] {
            assert_eq!(closures(&[tag]), vec![], "{tag:?}");
        }
    }
}
